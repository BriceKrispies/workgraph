"""Stack verification: the full tier on a stack's integration tip, and bisection when it fails.

Once the queue settles (no item pending or ready, at least one item on the integration branch), a
``stack_verify`` job runs the parent task's ``full`` checks in each integration worktree at its tip.
Passing verifies every merged item and moves its child to ``awaiting_review``.

Failing bisects the merge sequence. A probe checks out one prefix of the sequence in a detached
bisect worktree per repository and runs only the checks that failed on the tip. A check in regression
mode fails only on signatures the stack's base does not already report, so a red base does not block
the stack. A check that errors (it could not judge its revision, see ``error_exit_codes``) on the tip,
on the base or on a probe stops the verification with ``stack.verification_errored``: nothing is
ejected and nothing passes. The first item whose prefix fails is ejected: its task returns to ready with the failure,
the items that depend on it go back to pending, and the integration branch is rebuilt from the base
without it. Verification then runs again on the rebuilt tip. A failure on the base itself ejects
nothing.
"""

from __future__ import annotations

import shutil
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConflictError,
    GitError,
    InvalidTransitionError,
    NotFoundError,
    ValidationFailed,
)
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    INACTIVE_STACK_STATUSES,
    StackItemState,
    StackStatus,
    TaskStatus,
    VerificationStatus,
    WorktreeStatus,
)
from workgraph.core.models.schemas import CommentCreate
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.stacks import latest_event_id
from workgraph.integrations.git.cli import Git
from workgraph.integrations.git.regression import CheckOutcome, regressions
from workgraph.integrations.git.stack_integration import (
    RETURNABLE_TASK_STATUSES,
    IntegrationTarget,
    MergeOutcome,
    StackIntegrator,
    directory_name,
    dump_detail,
    load_detail,
    merge_head,
)
from workgraph.jobs import queue as jobqueue
from workgraph.jobs.limits import stack_key, verify_key
from workgraph.persistence.orm import (
    ManagedWorktree,
    Repository,
    RevisionManifest,
    Stack,
    StackItem,
    StackRepository,
    Task,
    TaskDependency,
    VerificationCheckResult,
    VerificationRun,
)
from workgraph.security.permissions import Permission, require
from workgraph.verification.configs import ApplicableCheck, CheckSpec, applicable_checks
from workgraph.verification.service import CheckResultOut, VerificationRunOut

# Items whose merges are on the integration branch. A conflicting item may hold merges in the
# repositories that did integrate, so it stays in the merge sequence until it is fixed.
ON_BRANCH_STATES = frozenset({StackItemState.MERGED, StackItemState.VERIFIED, StackItemState.CONFLICT})


@dataclass(frozen=True)
class StackRepoBase:
    repository_id: str
    name: str
    base_sha: str


@dataclass(frozen=True)
class _Entry:
    """One item on the integration branch, in merge order."""

    item_id: str
    task_id: str
    task_key: str
    title: str
    merges: dict[str, dict[str, str]]


def _in_scope(repos: list[StackRepoBase]) -> Callable[[ApplicableCheck], bool]:
    """Full checks that apply to this stack: workspace and project checks, and checks of its repositories."""
    names = {repo.name for repo in repos} | {repo.repository_id for repo in repos}

    def keep(check: ApplicableCheck) -> bool:
        return check.spec.tier == "full" and (check.spec.repository is None or check.spec.repository in names)

    return keep


def _prefix_shas(bases: dict[str, str], sequence: list[_Entry], position: int) -> dict[str, str]:
    """The commit each repository holds after the first ``position`` items of the merge sequence."""
    shas = dict(bases)
    for entry in sequence[:position]:
        for name, merge in entry.merges.items():
            shas[name] = merge["merge_commit"]
    return shas


def _at_bases(entries: list[dict[str, Any]], repos: list[StackRepoBase]) -> bool:
    """Whether a revision manifest holds exactly the base commit of every stack repository."""
    shas = {entry.get("repository_id"): entry.get("sha") for entry in entries}
    return all(shas.get(repo.repository_id) == repo.base_sha for repo in repos)


# How many of the workspace's most recent verification runs are scanned for a reusable base outcome.
_CACHE_SCAN_LIMIT = 500


def _failed_checks(run: VerificationRunOut) -> dict[str, str | None]:
    """Required checks that did not pass, each with the repository its result was recorded against."""
    return {
        check.name: check.repository_id for check in run.checks if check.required and check.status != "passed"
    }


def _errored_checks(run: VerificationRunOut) -> list[CheckResultOut]:
    """Required checks that could not judge anything. A run with one proves nothing about its revision."""
    return [check for check in run.checks if check.required and check.status == VerificationStatus.ERROR]


def _tail(excerpt: str, lines: int = 20) -> str:
    return "\n".join(excerpt.splitlines()[-lines:])


def _is_absolute(specs: dict[str, CheckSpec], name: str) -> bool:
    spec = specs.get(name)
    return spec is None or spec.baseline == "absolute"


def _ran_with(run: VerificationRun, name: str, spec: CheckSpec) -> bool:
    """Whether ``run`` ran the check ``name`` with exactly the current definition (its snapshot entry)."""
    current = spec.model_dump(mode="json")
    return any(
        {key: value for key, value in entry.items() if key not in ("config_id", "config")} == current
        for entry in run.config_snapshot
        if entry.get("name") == name
    )


class StackVerifier(Service):
    # ------------------------------------------------------------------ requests

    def request(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> dict[str, Any]:
        """Queue verification of the parent's active stack, on demand."""
        require(ctx, Permission.STACK_WRITE)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = session.scalar(
                select(Stack).where(
                    Stack.parent_task_id == parent.id, Stack.status.not_in(INACTIVE_STACK_STATUSES)
                )
            )
            if stack is None:
                raise NotFoundError(f"{parent.key} has no active stack", details={"parent": parent.key})
            stack_id, parent_key = stack.id, parent.key
            version = latest_event_id(session, stack)
        job_id = self._enqueue(ctx, stack_id, version=version)
        assert job_id is not None
        return {"stack": parent_key, "job_id": job_id}

    def enqueue_if_settled(self, ctx: RequestContext, stack_id: str) -> str | None:
        """Queue verification once no item is pending or ready; repeated calls for the same tips collapse."""
        with self.db.session() as session:
            stack = session.get(Stack, stack_id)
            if stack is None or not self._settled(session, stack):
                return None
        return self._enqueue(ctx, stack_id, version=None)

    def _enqueue(self, ctx: RequestContext, stack_id: str, *, version: str | None) -> str | None:
        """Queue one ``stack_verify`` job keyed by the integration tips (and the stack version, on demand)."""
        with self.db.session() as session:
            stack = session.get(Stack, stack_id)
            if stack is None or stack.status in INACTIVE_STACK_STATUSES:
                return None
            repos = self._repos(session, stack)
        tips = self._tips(stack_id, repos)
        with self.db.session(write=True) as session:
            stack = session.get(Stack, stack_id)
            assert stack is not None
            key = f"stack-verify:{stack_id}:" + ",".join(f"{repo.name}={tips[repo.name]}" for repo in repos)
            if version:
                key += f":{version}"
            # This stack's keys only, so other stacks on the repository keep integrating. ``verify:`` bounds
            # how many heavy verifications run on one repository at once.
            resource_keys = sorted(
                {stack_key(stack_id, repo.repository_id) for repo in repos}
                | {verify_key(repo.repository_id) for repo in repos}
            )
            job = jobqueue.enqueue(
                session,
                "stack_verify",
                {"stack_id": stack_id},
                workspace_id=stack.workspace_id,
                idempotency_key=key,
                resource_keys=resource_keys,
                causation_id=version or latest_event_id(session, stack),
                created_by=ctx.actor,
            )
            return job.id

    # ------------------------------------------------------------------ job

    def verify(self, ctx: RequestContext, stack_id: str) -> dict[str, Any]:
        """Verify the integration tip once the stack has settled; bisect and eject on failure."""
        with self.db.session() as session:
            stack = session.get(Stack, stack_id)
            if stack is None or stack.status in INACTIVE_STACK_STATUSES:
                return {"status": "inactive"}
            if not self._settled(session, stack):
                return {"status": "waiting"}
            parent = session.get(Task, stack.parent_task_id)
            assert parent is not None
            parent_id = parent.id
            workspace_id = stack.workspace_id
            repos = self._repos(session, stack)
            keep = _in_scope(repos)
            checks = [check for check in applicable_checks(session, parent) if keep(check)]
        # A default branch that moved since the stack was cut is rebuilt first, so the full checks run once,
        # on the tip that lands. The rebuild queues a new verification.
        if self.app.stack_lander.rebase_if_stale(ctx, stack_id):
            return {"status": "rebased"}
        targets: list[IntegrationTarget] = []
        paths: dict[str, Path] = {}
        tips: dict[str, str] = {}
        for repo in repos:
            target = self.app.stack_integration.target(stack_id, repo.repository_id)
            if target is None:
                return {"status": "inactive"}
            path = self.app.stack_integration.ensure_worktree(ctx, target)
            tip = Git(path).head()
            if tip is None:
                raise GitError(f"integration worktree {path} has no HEAD")
            targets.append(target)
            paths[repo.name] = path
            tips[repo.name] = tip
        with self.db.session(write=True) as session:
            stack = session.get(Stack, stack_id)
            assert stack is not None
            self.set_status(session, ctx, stack, StackStatus.VERIFYING, reason="verification started")
            record_event(
                session,
                ctx,
                event_type="stack.verification_started",
                entity_type="stack",
                entity_id=stack_id,
                workspace_id=stack.workspace_id,
                data={"tips": tips, "checks": [check.spec.name for check in checks]},
            )
        if not checks:
            return self._passed(ctx, stack_id, run_id=None, tips=tips, reason="no full checks are configured")
        run = self.app.verification.run(
            ctx,
            None,
            parent_id,
            manifest_entries=self._entries(targets, paths, tips, branch=True),
            tier="full",
            check_filter=keep,
            event_context={"stack": {"id": stack_id, "tips": tips}},
        )
        for path in paths.values():
            Git(path).run("clean", "-ffdx", check=False)
        errored = _errored_checks(run)
        if errored:
            return self._verification_errored(
                ctx,
                stack_id,
                run_id=run.id,
                errors={check.name: _tail(check.output_excerpt) for check in errored},
                stage="tip",
            )
        if run.required_passed:
            return self._passed(ctx, stack_id, run_id=run.id, tips=tips, reason="full checks passed")
        specs = {check.spec.name: check.spec for check in checks}
        failed = _failed_checks(run)
        base = self._base_outcomes(ctx, stack_id, parent_id, workspace_id, repos, targets, failed, specs)
        base_errors = {
            name: outcome for name, outcome in base.items() if outcome.status == VerificationStatus.ERROR
        }
        if base_errors:
            return self._verification_errored(
                ctx,
                stack_id,
                run_id=next(iter(base_errors.values())).run_id,
                errors={name: _tail(outcome.excerpt) for name, outcome in base_errors.items()},
                stage="base",
            )
        if not regressions(run, base, specs):
            tolerated = {
                check.name: list(check.failure_signatures) for check in run.checks if check.name in failed
            }
            return self._passed(
                ctx,
                stack_id,
                run_id=run.id,
                tips=tips,
                reason="no new failures against the base",
                extra={"tolerated": tolerated},
            )
        return self._bisect(ctx, stack_id, parent_id, repos, targets, run, tips, base, specs)

    # ------------------------------------------------------------------ errored

    def _verification_errored(
        self, ctx: RequestContext, stack_id: str, *, run_id: str, errors: dict[str, str], stage: str
    ) -> dict[str, Any]:
        """Stop when a check could not judge its revision: no pass, no bisection, no ejection.

        ``errors`` maps each errored check to the tail of its output. The stack returns to ``integrating``
        so that a new verification (or a fix to the check) can run; no item changes state.
        """
        names = sorted(errors)
        reason = f"verification could not judge {', '.join(names)} (run {run_id}, {stage})"
        with self.db.session(write=True) as session:
            stack, _ = self._load(session, stack_id)
            record_event(
                session,
                ctx,
                event_type="stack.verification_errored",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={"run_id": run_id, "stage": stage, "checks": names, "excerpt": errors, "reason": reason},
            )
            self.set_status(session, ctx, stack, StackStatus.INTEGRATING, reason=reason)
        return {"status": "errored", "run_id": run_id, "checks": names, "stage": stage}

    # ------------------------------------------------------------------ pass

    def _passed(
        self,
        ctx: RequestContext,
        stack_id: str,
        *,
        run_id: str | None,
        tips: dict[str, str],
        reason: str,
        extra: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        with self.db.session(write=True) as session:
            stack, parent = self._load(session, stack_id)
            rows = self.item_rows(session, stack)
            verified = []
            for item, task in rows:
                if StackItemState(item.state) == StackItemState.MERGED:
                    self.app.stacks.transition_item(
                        session, ctx, stack, parent, task, item, StackItemState.VERIFIED, item.detail
                    )
                    verified.append(task.key)
            for item, task in rows:
                if (
                    StackItemState(item.state) == StackItemState.VERIFIED
                    and TaskStatus(task.status) == TaskStatus.AWAITING_VERIFICATION
                ):
                    where = f" (verification run {run_id})" if run_id else ""
                    self.app.tasks._transition(
                        session,
                        ctx,
                        task,
                        TaskStatus.AWAITING_REVIEW,
                        reason=f"stack {parent.key} verified on the integration tip{where}",
                        extra={"run_id": run_id},
                    )
            self._review_parent(session, ctx, stack, parent, run_id)
            self.set_status(session, ctx, stack, StackStatus.VERIFIED, reason=reason)
            record_event(
                session,
                ctx,
                event_type="stack.verified",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={"run_id": run_id, "tips": tips, "tasks": verified, "reason": reason, **(extra or {})},
            )
        landing = self.app.stack_lander.after_verified(ctx, stack_id)
        return {"status": "verified", "run_id": run_id, "verified": verified, "landing": landing}

    def _review_parent(
        self, session: Session, ctx: RequestContext, stack: Stack, parent: Task, run_id: str | None
    ) -> None:
        """Make the parent reviewable: the stack's full run on the integration tip verifies the parent.

        The parent is walked to ``awaiting_review`` one legal lifecycle step at a time. A parent that
        cannot get there (for example, with unmet dependencies) stays where it is and the reason is
        recorded, so the stack still verifies and the parent can be moved by hand.
        """
        steps = {
            TaskStatus.AWAITING_VERIFICATION: [TaskStatus.AWAITING_REVIEW],
            TaskStatus.CLAIMED: [TaskStatus.AWAITING_REVIEW],
            TaskStatus.RUNNING: [TaskStatus.AWAITING_REVIEW],
            TaskStatus.READY: [TaskStatus.CLAIMED, TaskStatus.AWAITING_REVIEW],
            TaskStatus.BACKLOG: [TaskStatus.READY, TaskStatus.CLAIMED, TaskStatus.AWAITING_REVIEW],
            TaskStatus.BLOCKED: [TaskStatus.READY, TaskStatus.CLAIMED, TaskStatus.AWAITING_REVIEW],
            TaskStatus.FAILED: [TaskStatus.READY, TaskStatus.CLAIMED, TaskStatus.AWAITING_REVIEW],
        }
        if TaskStatus(parent.status) == TaskStatus.AWAITING_REVIEW:
            return
        reason = f"stack {parent.key} verified on the integration tip" + (
            f" (verification run {run_id})" if run_id else ""
        )
        try:
            for step in steps.get(TaskStatus(parent.status), []):
                self.app.tasks._transition(
                    session, ctx, parent, step, reason=reason, extra={"run_id": run_id}
                )
            if TaskStatus(parent.status) != TaskStatus.AWAITING_REVIEW:
                raise InvalidTransitionError(f"{parent.key} is {parent.status} and cannot be reviewed yet")
        except InvalidTransitionError as exc:
            record_event(
                session,
                ctx,
                event_type="stack.parent_not_reviewable",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={"task_key": parent.key, "status": parent.status, "reason": exc.message},
            )

    # ------------------------------------------------------------------ bisect

    def _bisect(
        self,
        ctx: RequestContext,
        stack_id: str,
        parent_id: str,
        repos: list[StackRepoBase],
        targets: list[IntegrationTarget],
        tip_run: VerificationRunOut,
        tips: dict[str, str],
        base: dict[str, CheckOutcome],
        specs: dict[str, CheckSpec],
    ) -> dict[str, Any]:
        failing = regressions(tip_run, base, specs)
        failed = _failed_checks(tip_run)
        # An absolute check that fails on the base is a failure of the base itself, not of any child.
        on_base = sorted(
            name
            for name in failed
            if _is_absolute(specs, name) and (name not in base or base[name].status != "passed")
        )
        with self.db.session(write=True) as session:
            stack, _ = self._load(session, stack_id)
            sequence = self._sequence(session, stack)
            record_event(
                session,
                ctx,
                event_type="stack.verification_failed",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={
                    "run_id": tip_run.id,
                    "tips": tips,
                    "failed_checks": sorted(failing),
                    "signatures": failing,
                },
            )
        if not sequence or on_base:
            run_id = tip_run.id if not sequence else base[on_base[0]].run_id
            reason = "no merged change to eject" if not sequence else "the base fails"
            with self.db.session(write=True) as session:
                stack, _ = self._load(session, stack_id)
                record_event(
                    session,
                    ctx,
                    event_type="stack.verification_failed_on_base",
                    entity_type="stack",
                    entity_id=stack.id,
                    workspace_id=stack.workspace_id,
                    data={"run_id": run_id, "failed_checks": sorted(on_base or failing), "reason": reason},
                )
                self.set_status(session, ctx, stack, StackStatus.INTEGRATING, reason=reason)
            return {"status": "failed_on_base", "run_id": run_id}

        in_scope = _in_scope(repos)
        bases = {repo.name: repo.base_sha for repo in repos}
        # Invariant: the prefix ``low`` passes and the prefix ``high`` fails. The base passes by construction.
        low, high, high_run = 0, len(sequence), tip_run
        with self._worktrees(ctx, targets) as bisect:
            while high - low > 1:
                middle = (low + high) // 2
                shas = _prefix_shas(bases, sequence, middle)
                run = self._probe(
                    ctx, stack_id, parent_id, targets, bisect, shas, set(failing), in_scope, middle
                )
                errored = _errored_checks(run)
                if errored:
                    return self._verification_errored(
                        ctx,
                        stack_id,
                        run_id=run.id,
                        errors={check.name: _tail(check.output_excerpt) for check in errored},
                        stage="bisect",
                    )
                passed = not regressions(run, base, specs)
                with self.db.session(write=True) as session:
                    stack, _ = self._load(session, stack_id)
                    record_event(
                        session,
                        ctx,
                        event_type="stack.bisect_probe",
                        entity_type="stack",
                        entity_id=stack.id,
                        workspace_id=stack.workspace_id,
                        data={
                            "position": middle,
                            "shas": shas,
                            "passed": passed,
                            "run_id": run.id,
                            "failed_checks": sorted(
                                c.name for c in run.checks if c.required and c.status != "passed"
                            ),
                        },
                    )
                if passed:
                    low = middle
                else:
                    high, high_run = middle, run
        culprit = sequence[high - 1]
        task_key = self._eject(ctx, stack_id, culprit, high, high_run, regressions(high_run, base, specs))
        self.rebuild(ctx, stack_id, targets, repos)
        self._enqueue(ctx, stack_id, version=None)
        return {"status": "ejected", "task": task_key, "position": high, "run_id": high_run.id}

    def _eject(
        self,
        ctx: RequestContext,
        stack_id: str,
        culprit: _Entry,
        position: int,
        run: VerificationRunOut,
        signatures: dict[str, list[str]],
    ) -> str:
        ejection = {
            "position": position,
            "verification_run_id": run.id,
            "failed_checks": sorted(signatures),
            "signatures": signatures,
        }
        names = ", ".join(sorted(signatures))
        listed = "; ".join(
            f"{name}: {', '.join(found) or 'none reported'}" for name, found in signatures.items()
        )
        comment = (
            f"Stack {{parent}} verification failed on the integration tip, and bisection "
            f"placed the failure at {{task}} (position {position} in the merge order). "
            f"Failing checks: {names}. Failure signatures: {listed or 'none reported'}. "
            f"Verification run: {run.id}. Next step: fix the failure in this change and submit "
            "a new attempt; the stack verifies again once the change is integrated."
        )
        return self._apply_ejection(ctx, culprit, ejection, comment, reason=names)

    def eject(
        self, ctx: RequestContext, workspace: str | None, parent_ref: str, task_ref: str, reason: str
    ) -> dict[str, Any]:
        """Eject a merged child an operator has identified as a stack's failure, then rebuild without it.

        For when the culprit is known from evidence and bisection would cost full runs, or would be
        misled by an unrelated flaky failure. The child goes back to ``ready`` like a bisected one.
        """
        require(ctx, Permission.STACK_WRITE)
        if not reason.strip():
            raise ValidationFailed("an ejection needs a reason (the evidence that names this child)")
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            child = resolve.task(session, ws_id, task_ref)
            stack = session.scalar(
                select(Stack).where(
                    Stack.parent_task_id == parent.id, Stack.status.not_in(INACTIVE_STACK_STATUSES)
                )
            )
            if stack is None:
                raise NotFoundError(f"{parent.key} has no active stack", details={"parent": parent.key})
            stack_id = stack.id
            culprit = next((e for e in self._sequence(session, stack) if e.task_key == child.key), None)
            if culprit is None:
                raise ConflictError(
                    f"{child.key} is not merged on {parent.key}'s stack; only a merged child can be ejected",
                    details={"task": child.key},
                )
        ejection = {"manual": True, "reason": reason}
        comment = f"Ejected from stack {{parent}} by {ctx.actor}: {reason}"
        task_key = self._apply_ejection(ctx, culprit, ejection, comment, reason=reason)
        self.app.stack_lander.rebuild_without(ctx, stack_id)
        return {"status": "ejected", "task": task_key, "stack_id": stack_id}

    def _apply_ejection(
        self, ctx: RequestContext, culprit: _Entry, ejection: dict[str, Any], comment: str, *, reason: str
    ) -> str:
        """Mark ``culprit`` ejected, return its task to ready with ``comment``, and park its dependents."""
        names = reason
        with self.db.session(write=True) as session:
            stack, item, task, parent = StackIntegrator.load_item(session, culprit.item_id)
            self.app.stacks.transition_item(
                session,
                ctx,
                stack,
                parent,
                task,
                item,
                StackItemState.EJECTED,
                dump_detail({"merges": {}, "conflicts": {}, "ejection": ejection}),
            )
            record_event(
                session,
                ctx,
                event_type="stack.item_ejected",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={"task_key": task.key, **ejection},
            )
            if TaskStatus(task.status) in RETURNABLE_TASK_STATUSES:
                self.app.tasks._transition(
                    session, ctx, task, TaskStatus.READY, reason=f"ejected from stack {parent.key}: {names}"
                )
            self.app.tasks._comment(
                session,
                ctx,
                task,
                CommentCreate(
                    body=comment.format(parent=parent.key, task=task.key),
                    data={"stack_item_ejection": ejection},
                ),
            )
            for dependent_item, dependent_task in self.dependents(session, stack, task.id):
                if StackItemState(dependent_item.state) == StackItemState.EJECTED:
                    continue
                reason = f"dependency {task.key} was ejected"
                self.app.stacks.transition_item(
                    session,
                    ctx,
                    stack,
                    parent,
                    dependent_task,
                    dependent_item,
                    StackItemState.PENDING,
                    dump_detail({"merges": {}, "conflicts": {}, "reason": reason}),
                )
        return task.key

    # ------------------------------------------------------------------ rebuild

    def rebuild(
        self, ctx: RequestContext, stack_id: str, targets: list[IntegrationTarget], repos: list[StackRepoBase]
    ) -> None:
        """Reset each integration branch to its base and replay the items still on it, in merge order."""
        with self.db.session() as session:
            stack, _ = self._load(session, stack_id)
            sequence = self._sequence(session, stack)
        by_name = {target.repository_name: target for target in targets}
        bases = {repo.name: repo.base_sha for repo in repos}
        results: dict[tuple[str, str], tuple[str, MergeOutcome]] = {}
        tips: dict[str, str] = {}
        for name in sorted(by_name):
            target = by_name[name]
            git = Git(self.app.stack_integration.ensure_worktree(ctx, target))
            git.run("clean", "-ffdx")
            git.run("reset", "--hard", bases[name])
            for entry in sequence:
                if name not in entry.merges:
                    continue
                head = entry.merges[name]["head"]
                message = f"Merge {entry.task_key}: {entry.title} into {target.integration_branch}"
                results[(entry.item_id, name)] = (head, merge_head(git, head, message))
            tip = git.head()
            assert tip is not None
            tips[name] = tip
        replayed: list[str] = []
        conflicted: list[str] = []
        with self.db.session(write=True) as session:
            stack, _ = self._load(session, stack_id)
            for entry in sequence:
                stack, item, task, parent = StackIntegrator.load_item(session, entry.item_id)
                detail = load_detail(item.detail)
                failures = []
                for name in entry.merges:
                    head, outcome = results[(entry.item_id, name)]
                    if outcome.conflict_paths is None:
                        detail["merges"][name] = {"head": head, "merge_commit": outcome.merge_commit}
                        detail["conflicts"].pop(name, None)
                    else:
                        detail["merges"].pop(name, None)
                        failures.append((name, head, outcome))
                item.detail = dump_detail(detail)
                if not failures:
                    replayed.append(entry.task_key)
                for name, head, outcome in failures:
                    conflicted.append(entry.task_key)
                    self.app.stack_integration.record_conflict(
                        session,
                        ctx,
                        stack,
                        parent,
                        task,
                        item,
                        repository=name,
                        branch=by_name[name].integration_branch,
                        head=head,
                        outcome=outcome,
                        detail=detail,
                    )
            self.set_status(
                session, ctx, stack, StackStatus.INTEGRATING, reason="rebuilt without ejected items"
            )
            record_event(
                session,
                ctx,
                event_type="stack.rebuilt",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={"base_shas": bases, "tips": tips, "replayed": replayed, "conflicted": conflicted},
            )

    # ------------------------------------------------------------------ worktrees

    def _bisect_worktree(self, ctx: RequestContext, target: IntegrationTarget) -> tuple[str, Path]:
        """A detached worktree for probing prefixes of one repository; returns its row id and path."""
        path = (
            self.app.paths.worktrees_dir
            / "stacks"
            / target.stack_id
            / f"bisect-{directory_name(target.repository_name)}"
        )
        path.parent.mkdir(parents=True, exist_ok=True)
        source = Git(target.source_path)
        source.run("worktree", "prune", check=False)
        if path.exists():
            source.run("worktree", "remove", "--force", str(path), check=False)
            shutil.rmtree(path, ignore_errors=True)
        source.run("worktree", "add", "--detach", str(path), target.base_sha)
        with self.db.session(write=True) as session:
            row = session.scalar(select(ManagedWorktree).where(ManagedWorktree.path == str(path)))
            if row is None:
                row = ManagedWorktree(
                    workspace_id=target.workspace_id,
                    repository_id=target.repository_id,
                    source_checkout_id=target.checkout_id,
                    attempt_id=None,
                    path=str(path),
                    branch="",
                    base_sha=target.base_sha,
                )
                session.add(row)
            row.status = WorktreeStatus.ACTIVE
            row.base_sha = target.base_sha
            row.removed_at = None
            session.flush()
            record_event(
                session,
                ctx,
                event_type="worktree.created",
                entity_type="worktree",
                entity_id=row.id,
                workspace_id=target.workspace_id,
                data={
                    "purpose": "stack_bisect",
                    "stack_id": target.stack_id,
                    "repository": target.repository_name,
                    "path": str(path),
                    "base_sha": target.base_sha,
                },
            )
            return row.id, path

    # ------------------------------------------------------------------ helpers

    @staticmethod
    def _entries(
        targets: list[IntegrationTarget], paths: dict[str, Path], shas: dict[str, str], *, branch: bool
    ) -> list[dict[str, Any]]:
        return [
            {
                "repository_id": target.repository_id,
                "repository": target.repository_name,
                "checkout_path": str(paths[target.repository_name]),
                "sha": shas[target.repository_name],
                "ref": target.integration_branch if branch else None,
                "access": "read_write",
            }
            for target in targets
        ]

    def _base_outcomes(
        self,
        ctx: RequestContext,
        stack_id: str,
        parent_id: str,
        workspace_id: str,
        repos: list[StackRepoBase],
        targets: list[IntegrationTarget],
        failed: dict[str, str | None],
        specs: dict[str, CheckSpec],
    ) -> dict[str, CheckOutcome]:
        """How each failed check finished on the stack's base: from an earlier run at the base, else a new run.

        ``failed`` maps each check name to the repository its tip result was recorded against. A new base
        run that errors is returned as an ``error`` outcome, and the caller stops on it.
        """
        if not failed:
            return {}
        outcomes = self._cached_outcomes(workspace_id, repos, failed, specs)
        missing = {name for name in failed if name not in outcomes}
        if missing:
            bases = {repo.name: repo.base_sha for repo in repos}
            with self._worktrees(ctx, targets) as bisect:
                run = self._probe(
                    ctx, stack_id, parent_id, targets, bisect, bases, missing, _in_scope(repos), position=0
                )
            for check in run.checks:
                if check.name in missing and check.repository_id == failed[check.name]:
                    outcomes[check.name] = CheckOutcome(
                        check.status, list(check.failure_signatures), run.id, check.output_excerpt
                    )
        return outcomes

    def _cached_outcomes(
        self,
        workspace_id: str,
        repos: list[StackRepoBase],
        failed: dict[str, str | None],
        specs: dict[str, CheckSpec],
    ) -> dict[str, CheckOutcome]:
        """Outcomes of the failed checks from the newest runs in the workspace whose manifest is at the bases.

        Only the most recent ``_CACHE_SCAN_LIMIT`` runs are scanned. A result is taken only when the check
        name and the repository it was recorded against match, the result did not error, and the run ran
        the check with the current definition (same command, env, parser, baseline and so on).
        """
        found: dict[str, CheckOutcome] = {}
        with self.db.session() as session:
            runs = session.scalars(
                select(VerificationRun)
                .where(VerificationRun.workspace_id == workspace_id)
                .order_by(VerificationRun.created_at.desc())
                .limit(_CACHE_SCAN_LIMIT)
            ).all()
            for run in runs:
                pending = [name for name in failed if name not in found and name in specs]
                if not pending:
                    break
                manifest = session.get(RevisionManifest, run.manifest_id) if run.manifest_id else None
                if manifest is None or not _at_bases(manifest.entries, repos):
                    continue
                rows = session.scalars(
                    select(VerificationCheckResult).where(
                        VerificationCheckResult.run_id == run.id, VerificationCheckResult.name.in_(pending)
                    )
                )
                for row in rows:
                    if row.name in found or row.repository_id != failed[row.name]:
                        continue
                    if row.status == VerificationStatus.ERROR or not _ran_with(
                        run, row.name, specs[row.name]
                    ):
                        continue
                    found[row.name] = CheckOutcome(
                        row.status, list(row.failure_signatures), run.id, row.output_excerpt
                    )
        return found

    def _probe(
        self,
        ctx: RequestContext,
        stack_id: str,
        parent_id: str,
        targets: list[IntegrationTarget],
        bisect: dict[str, tuple[str, Path]],
        shas: dict[str, str],
        names: set[str],
        in_scope: Callable[[ApplicableCheck], bool],
        position: int,
    ) -> VerificationRunOut:
        """Check out one revision in the bisect worktrees and run the named full checks there."""
        for name, sha in shas.items():
            git = Git(bisect[name][1])
            git.run("checkout", "--force", "--detach", sha)
            git.run("clean", "-ffdx")
        return self.app.verification.run(
            ctx,
            None,
            parent_id,
            manifest_entries=self._entries(
                targets, {name: path for name, (_, path) in bisect.items()}, shas, branch=False
            ),
            tier="full",
            check_filter=lambda check: in_scope(check) and check.spec.name in names,
            event_context={"stack": {"id": stack_id, "position": position, "shas": shas}},
        )

    @contextmanager
    def _worktrees(
        self, ctx: RequestContext, targets: list[IntegrationTarget]
    ) -> Iterator[dict[str, tuple[str, Path]]]:
        """Bisect worktrees for every target repository, removed when the block ends."""
        bisect: dict[str, tuple[str, Path]] = {}
        try:
            for target in targets:
                bisect[target.repository_name] = self._bisect_worktree(ctx, target)
            yield bisect
        finally:
            for row_id, _ in bisect.values():
                self.app.worktrees.remove(ctx, row_id, force=True)

    def _tips(self, stack_id: str, repos: list[StackRepoBase]) -> dict[str, str]:
        """The integration tip of each repository; its base while the worktree does not exist yet."""
        tips: dict[str, str] = {}
        for repo in repos:
            path = self.app.stack_integration.worktree_path(stack_id, repo.name)
            tips[repo.name] = (Git(path).head() if path.exists() else None) or repo.base_sha
        return tips

    @staticmethod
    def _repos(session: Session, stack: Stack) -> list[StackRepoBase]:
        rows = session.execute(
            select(StackRepository.repository_id, Repository.name, StackRepository.base_sha)
            .join(Repository, Repository.id == StackRepository.repository_id)
            .where(StackRepository.stack_id == stack.id)
            .order_by(Repository.name)
        ).all()
        return [StackRepoBase(*row) for row in rows]

    @staticmethod
    def _settled(session: Session, stack: Stack) -> bool:
        states = [
            StackItemState(state)
            for state in session.scalars(select(StackItem.state).where(StackItem.stack_id == stack.id))
        ]
        return not any(state in (StackItemState.PENDING, StackItemState.READY) for state in states) and any(
            state in (StackItemState.MERGED, StackItemState.VERIFIED) for state in states
        )

    @staticmethod
    def _load(session: Session, stack_id: str) -> tuple[Stack, Task]:
        stack = session.get(Stack, stack_id)
        if stack is None:
            raise NotFoundError(f"stack {stack_id} not found")
        parent = session.get(Task, stack.parent_task_id)
        assert parent is not None
        return stack, parent

    @staticmethod
    def item_rows(session: Session, stack: Stack) -> list[tuple[StackItem, Task]]:
        return [
            tuple(row)
            for row in session.execute(
                select(StackItem, Task)
                .join(Task, Task.id == StackItem.task_id)
                .where(StackItem.stack_id == stack.id)
                .order_by(StackItem.merge_order.is_(None), StackItem.merge_order, Task.number)
            ).all()
        ]

    @staticmethod
    def _sequence(session: Session, stack: Stack) -> list[_Entry]:
        """The items whose merges are on the integration branch, in merge order."""
        entries = []
        for item, task in StackVerifier.item_rows(session, stack):
            if StackItemState(item.state) not in ON_BRANCH_STATES:
                continue
            merges = load_detail(item.detail)["merges"]
            if merges:
                entries.append(_Entry(item.id, task.id, task.key, task.title, merges))
        return entries

    @staticmethod
    def dependents(session: Session, stack: Stack, task_id: str) -> list[tuple[StackItem, Task]]:
        """Items of the stack that depend, directly or through other items, on one task."""
        rows = {task.id: (item, task) for item, task in StackVerifier.item_rows(session, stack)}
        edges = session.execute(
            select(TaskDependency.task_id, TaskDependency.depends_on_id).where(
                TaskDependency.task_id.in_(list(rows)), TaskDependency.depends_on_id.in_(list(rows))
            )
        ).all()
        dependents: dict[str, list[str]] = {}
        for dependent, prerequisite in edges:
            dependents.setdefault(prerequisite, []).append(dependent)
        found: list[str] = []
        seen = {task_id}
        frontier = [task_id]
        while frontier:
            for dependent in dependents.get(frontier.pop(), []):
                if dependent not in seen:
                    seen.add(dependent)
                    found.append(dependent)
                    frontier.append(dependent)
        return [rows[task] for task in sorted(found, key=lambda t: rows[t][1].number)]

    @staticmethod
    def set_status(
        session: Session, ctx: RequestContext, stack: Stack, target: StackStatus, *, reason: str
    ) -> None:
        if stack.status == target:
            return
        previous = stack.status
        stack.status = target
        stack.updated_at = utcnow()
        session.flush()
        record_event(
            session,
            ctx,
            event_type="stack.status_changed",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={"from": previous, "to": target, "reason": reason},
        )
