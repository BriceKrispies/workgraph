"""Stack landing: accepting a verified stack's parent moves the default branch to the integration tip.

Each stack repository is checked before anything moves. When its default branch still equals the
stack's ``base_sha``, the branch fast-forwards to the verified integration tip: ``git merge --ff-only``
in the user checkout that holds the branch (refused when that checkout has uncommitted changes), or a
compare-and-swap ``update-ref`` when no checkout does. A refusal leaves every repository untouched.

When a default branch moved since the stack was cut, nothing lands. The stack is rebuilt from the new
head (its ``base_sha`` is updated and the children are replayed), the landing request is kept on the
stack, and a full verification is queued. A passing verification then lands the stack; a failing one
leaves it to the usual bisection with the request still pending.

Landing accepts every verified child (automated, linked to the stack's passing run) and then the
parent. A child accepted on its own is released from the stack instead, and if its commits were on
the integration branch the branch is rebuilt without them.
"""

from __future__ import annotations

import json
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, GitError, NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    INACTIVE_STACK_STATUSES,
    StackItemState,
    StackStatus,
    TaskStatus,
    WorktreeStatus,
)
from workgraph.core.services.base import Service
from workgraph.core.services.stacks import active_item
from workgraph.integrations.git.cli import Git
from workgraph.integrations.git.stack_integration import IntegrationTarget, dump_detail
from workgraph.integrations.git.stack_verification import StackRepoBase, StackVerifier
from workgraph.persistence.orm import (
    Event,
    Job,
    ManagedWorktree,
    Repository,
    Stack,
    StackRepository,
    Task,
    VerificationRun,
)
from workgraph.verification.service import AcceptanceOut

# Task states from which the acceptance service accepts a task.
ACCEPTABLE_TASK_STATUSES = (TaskStatus.AWAITING_REVIEW, TaskStatus.AWAITING_VERIFICATION)
# Jobs that rebuild or move a stack's integration branches; a request must not rebuild under them.
# Per stack: a landing waits only for integration work of the same stack, never for another stack's.
INTEGRATION_JOB_KINDS = ("stack_integrate", "stack_verify")
INTEGRATION_JOB_STATUSES = ("queued", "running")


class StackLander(Service):
    # ------------------------------------------------------------------ lookups

    @staticmethod
    def active_stack_of_parent(session: Session, task_id: str) -> Stack | None:
        return session.scalar(
            select(Stack).where(Stack.parent_task_id == task_id, Stack.status.not_in(INACTIVE_STACK_STATUSES))
        )

    @staticmethod
    def _integration_busy(session: Session, stack_id: str) -> bool:
        payloads = session.scalars(
            select(Job.payload).where(
                Job.kind.in_(INTEGRATION_JOB_KINDS), Job.status.in_(INTEGRATION_JOB_STATUSES)
            )
        )
        return any(payload.get("stack_id") == stack_id for payload in payloads)

    @staticmethod
    def _request(stack: Stack) -> dict[str, Any] | None:
        return json.loads(stack.land_request) if stack.land_request else None

    def _targets(self, stack_id: str) -> list[IntegrationTarget]:
        """The integration target of every repository of the stack, by repository name."""
        with self.db.session() as session:
            repo_ids = list(
                session.scalars(
                    select(StackRepository.repository_id).where(StackRepository.stack_id == stack_id)
                )
            )
        targets = []
        for repo_id in repo_ids:
            target = self.app.stack_integration.target(stack_id, repo_id)
            if target is None:
                raise ConflictError(f"stack {stack_id} is no longer active", details={"stack_id": stack_id})
            targets.append(target)
        return sorted(targets, key=lambda target: target.repository_name)

    # ------------------------------------------------------------------ child acceptance

    def guard_child(self, session: Session, task: Task) -> None:
        """A child of an active stack leaves it only while no integration work for the stack is in flight."""
        found = active_item(session, task.id)
        if found is None:
            return
        stack, _ = found
        if self._integration_busy(session, stack.id):
            raise ConflictError(
                f"{task.key} is in a stack whose integration is queued or running; retry once it settles",
                details={"task": task.key, "stack_id": stack.id},
            )

    def release_child(self, session: Session, ctx: RequestContext, task: Task) -> str | None:
        """Remove an accepted child from its active stack.

        Returns the stack id when the child's commits were on the integration branch, so the caller
        rebuilds that branch without them. Its in-stack dependents go back to pending, as on ejection.
        """
        found = active_item(session, task.id)
        if found is None:
            return None
        stack, item = found
        parent = session.get(Task, stack.parent_task_id)
        assert parent is not None
        on_branch = StackItemState(item.state) in (StackItemState.MERGED, StackItemState.VERIFIED)
        if on_branch:
            for dependent, dependent_task in StackVerifier.dependents(session, stack, task.id):
                if StackItemState(dependent.state) == StackItemState.EJECTED:
                    continue
                self.app.stacks.transition_item(
                    session,
                    ctx,
                    stack,
                    parent,
                    dependent_task,
                    dependent,
                    StackItemState.PENDING,
                    dump_detail(
                        {
                            "merges": {},
                            "conflicts": {},
                            "reason": f"dependency {task.key} was accepted on its own",
                        }
                    ),
                )
        record_event(
            session,
            ctx,
            event_type="stack.item_released",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={"task_key": task.key, "parent_key": parent.key, "state": item.state, "rebuild": on_branch},
        )
        session.delete(item)
        stack.updated_at = utcnow()
        session.flush()
        return stack.id if on_branch else None

    def rebuild_without(self, ctx: RequestContext, stack_id: str | None) -> None:
        """Rebuild a stack's integration branches from their current base, after a release."""
        if stack_id is not None:
            self._rebuild(ctx, stack_id, bases=None)

    def _rebuild(self, ctx: RequestContext, stack_id: str, *, bases: dict[str, str] | None) -> None:
        targets = self._targets(stack_id)
        repos = [
            StackRepoBase(
                target.repository_id,
                target.repository_name,
                bases[target.repository_name] if bases else target.base_sha,
            )
            for target in targets
        ]
        self.app.stack_verification.rebuild(ctx, stack_id, targets, repos)
        self.app.stack_verification.enqueue_if_settled(ctx, stack_id)

    # ------------------------------------------------------------------ landing

    def after_verified(self, ctx: RequestContext, stack_id: str) -> str | None:
        """Land a stack that just verified, when a landing was requested or the workspace auto-accepts.

        Returns ``landed``, ``deferred`` (the stack was rebuilt for a moved default branch), ``refused``
        (the landing was refused and recorded) or None when nothing was asked for.
        """
        with self.db.session() as session:
            stack = session.get(Stack, stack_id)
            if stack is None or StackStatus(stack.status) != StackStatus.VERIFIED:
                return None
            request = self._request(stack)
            workspace_id = stack.workspace_id
        if (
            request is None
            and not self.app.effective_config(self.app.workspace_slug(workspace_id)).stacks.auto_accept
        ):
            return None
        mode, reason = (request["mode"], request["reason"]) if request else ("automated", "stack.auto_accept")
        try:
            accepted = self.land(ctx, stack_id, mode=mode, reason=reason, guard=False)
        except (ConflictError, ValidationFailed):
            return "refused"
        return "landed" if accepted is not None else "deferred"

    def land(
        self, ctx: RequestContext, stack_id: str, *, mode: str, reason: str, guard: bool = True
    ) -> AcceptanceOut | None:
        """Accept a verified stack's parent and land its children.

        Returns the parent's acceptance, or None when the default branch moved and the stack is being
        rebuilt to land once it verifies again. A refusal is recorded as an event and re-raised.
        ``guard`` refuses a rebuild while integration jobs for the stack are queued or running; a
        verification job itself passes False, since it already holds the integration resources.
        """
        try:
            return self._land(ctx, stack_id, mode=mode, reason=reason, guard=guard)
        except (ConflictError, ValidationFailed) as exc:
            with self.db.session(write=True) as session:
                stack = session.get(Stack, stack_id)
                if stack is not None:
                    record_event(
                        session,
                        ctx,
                        event_type="stack.landing_refused",
                        entity_type="stack",
                        entity_id=stack.id,
                        workspace_id=stack.workspace_id,
                        data={"mode": mode, "reason": reason, "error": exc.message},
                    )
            raise

    def _land(
        self, ctx: RequestContext, stack_id: str, *, mode: str, reason: str, guard: bool
    ) -> AcceptanceOut | None:
        with self.db.session() as session:
            stack = session.get(Stack, stack_id)
            if stack is None:
                raise NotFoundError(f"stack {stack_id} not found")
            parent = session.get(Task, stack.parent_task_id)
            assert parent is not None
            if StackStatus(stack.status) != StackStatus.VERIFIED:
                raise ConflictError(
                    f"{parent.key}'s stack is {stack.status}; only a verified stack can land",
                    details={"parent": parent.key, "stack_status": stack.status},
                )
            if TaskStatus(parent.status) not in ACCEPTABLE_TASK_STATUSES:
                raise ConflictError(
                    f"{parent.key} is {parent.status}; only completed work awaiting review can be accepted",
                    details={"parent": parent.key},
                )
            # Accepting the parent means the whole piece of work is done, so every child must land with it.
            unfinished = sorted(
                f"{task.key} ({item.state})"
                for item, task in StackVerifier.item_rows(session, stack)
                if StackItemState(item.state) != StackItemState.VERIFIED
            )
            if unfinished:
                raise ConflictError(
                    f"{parent.key}'s stack has unfinished children: {', '.join(unfinished)}; finish them or "
                    "remove them from the stack before accepting the parent",
                    details={"parent": parent.key, "unfinished": unfinished},
                )
            children = []
            for _item, task in StackVerifier.item_rows(session, stack):
                if TaskStatus(task.status) not in ACCEPTABLE_TASK_STATUSES:
                    raise ConflictError(
                        f"{task.key} is {task.status}; a verified child cannot be accepted in that state",
                        details={"task": task.key},
                    )
                children.append(task.key)
            defaults = dict(
                session.execute(
                    select(StackRepository.repository_id, Repository.default_branch)
                    .join(Repository, Repository.id == StackRepository.repository_id)
                    .where(StackRepository.stack_id == stack.id)
                ).all()
            )
            run_id, tips = self._verified_run(session, stack)
        targets = self._targets(stack_id)
        for target in targets:
            if not defaults.get(target.repository_id):
                raise ValidationFailed(f"repository {target.repository_name!r} has no default branch")
            integrated = Git(target.source_path).resolve_commit(target.integration_branch)
            if integrated != tips.get(target.repository_name):
                raise ConflictError(
                    f"the integration branch of {target.repository_name} moved since {parent.key} was verified; "
                    "verify the stack again before accepting it",
                    details={"parent": parent.key, "repository": target.repository_name},
                )
        heads = {
            target.repository_name: Git(target.source_path).resolve_commit(defaults[target.repository_id])
            for target in targets
        }
        if any(heads[target.repository_name] != target.base_sha for target in targets):
            return self._defer(ctx, stack_id, heads, mode=mode, reason=reason, guard=guard)
        plans = [(target, defaults[target.repository_id], tips[target.repository_name]) for target in targets]
        self._fast_forward(plans)
        return self._finish(ctx, stack_id, run_id=run_id, plans=plans, mode=mode, reason=reason)

    @staticmethod
    def _verified_run(session: Session, stack: Stack) -> tuple[str | None, dict[str, str]]:
        """The passing run and the integration tips the stack last verified."""
        event = session.scalar(
            select(Event)
            .where(Event.entity_id == stack.id, Event.event_type == "stack.verified")
            .order_by(Event.seq.desc())
            .limit(1)
        )
        if event is None:
            raise ConflictError(f"stack {stack.id} has no recorded verification")
        return event.data.get("run_id"), dict(event.data.get("tips", {}))

    def _defer(
        self,
        ctx: RequestContext,
        stack_id: str,
        heads: dict[str, str],
        *,
        mode: str,
        reason: str,
        guard: bool,
    ) -> None:
        """The default branch moved: rebase the stack onto it, keep the request, and verify again."""
        if guard:
            with self.db.session() as session:
                if self._integration_busy(session, stack_id):
                    raise ConflictError(
                        "the stack's integration is queued or running; retry once it settles",
                        details={"stack_id": stack_id},
                    )
        with self.db.session(write=True) as session:
            stack = session.get(Stack, stack_id)
            assert stack is not None
            parent = session.get(Task, stack.parent_task_id)
            assert parent is not None
            previous = {}
            for link, name in session.execute(
                select(StackRepository, Repository.name)
                .join(Repository, Repository.id == StackRepository.repository_id)
                .where(StackRepository.stack_id == stack.id)
            ).all():
                previous[name] = link.base_sha
                link.base_sha = heads[name]
            stack.land_request = json.dumps(
                {"mode": mode, "reason": reason, "actor": ctx.actor}, sort_keys=True
            )
            stack.updated_at = utcnow()
            session.flush()
            record_event(
                session,
                ctx,
                event_type="stack.land_requested",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={
                    "parent_key": parent.key,
                    "base_shas": previous,
                    "new_base_shas": heads,
                    "mode": mode,
                    "reason": reason,
                },
            )
        self._rebuild(ctx, stack_id, bases=heads)

    def _fast_forward(self, plans: list[tuple[IntegrationTarget, str, str]]) -> None:
        """Move each default branch from its base to the verified tip, after checking every repository.

        Every move is a fast-forward or a compare-and-swap on the branch's base, so a branch that another
        landing moved first is refused and the moves already made are rolled back.
        """
        moves: list[tuple[IntegrationTarget, str, str, Any]] = []
        for target, default, tip in plans:
            source = Git(target.source_path)
            if not source.is_ancestor(target.base_sha, tip):
                raise ConflictError(
                    f"the integration tip {tip} of {target.repository_name} does not descend from "
                    f"{default} at {target.base_sha}",
                    details={"repository": target.repository_name},
                )
            holder = next(
                (
                    entry.path
                    for entry in source.worktrees()
                    if entry.branch == default and entry.path.exists() and not entry.prunable
                ),
                None,
            )
            if holder is not None and not Git(holder).is_clean():
                raise ValidationFailed(
                    f"{holder} has uncommitted changes and has {default} checked out, so {default} cannot be "
                    "fast-forwarded there; commit or stash them, then accept again",
                    details={"repository": target.repository_name, "path": str(holder), "branch": default},
                )
            moves.append((target, default, tip, holder))
        moved: list[tuple[IntegrationTarget, str, str, Any]] = []
        try:
            for target, default, tip, holder in moves:
                if holder is not None:
                    Git(holder).run("merge", "--ff-only", tip)
                else:
                    Git(target.source_path).run("update-ref", f"refs/heads/{default}", tip, target.base_sha)
                moved.append((target, default, tip, holder))
        except GitError as exc:
            for target, default, tip, holder in reversed(moved):
                if holder is not None:
                    Git(holder).run("reset", "--keep", target.base_sha, check=False)
                else:
                    Git(target.source_path).run(
                        "update-ref", f"refs/heads/{default}", target.base_sha, tip, check=False
                    )
            # A default branch that moved under the landing (another stack's verification on the same
            # repository, when verify_concurrency allows it) is a refusal, not a crash of the job.
            raise ConflictError(
                "a default branch moved while the stack was landing; nothing was moved, verify and accept again"
            ) from exc

    def _finish(
        self,
        ctx: RequestContext,
        stack_id: str,
        *,
        run_id: str | None,
        plans: list[tuple[IntegrationTarget, str, str]],
        mode: str,
        reason: str,
    ) -> AcceptanceOut:
        """Record the acceptances, mark the stack landed, and remove its integration branches."""
        with self.db.session(write=True) as session:
            stack = session.get(Stack, stack_id)
            assert stack is not None
            parent = session.get(Task, stack.parent_task_id)
            assert parent is not None
            run = session.get(VerificationRun, run_id) if run_id else None
            accepted = []
            for item, task in StackVerifier.item_rows(session, stack):
                if StackItemState(item.state) != StackItemState.VERIFIED:
                    continue
                self.app.verification._accept(
                    session, ctx, task, mode="automated", reason=f"landed with stack {parent.key}", run=run
                )
                accepted.append(task.key)
            parent_acceptance = self.app.verification._accept(
                session, ctx, parent, mode=mode, reason=reason, run=run
            )
            StackVerifier.set_status(session, ctx, stack, StackStatus.LANDED, reason=f"{parent.key} accepted")
            stack.land_request = None
            record_event(
                session,
                ctx,
                event_type="stack.landed",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=stack.workspace_id,
                data={
                    "parent_key": parent.key,
                    "tips": {target.repository_name: tip for target, _, tip in plans},
                    "run_id": run_id,
                    "tasks": accepted,
                    "mode": mode,
                    "reason": reason,
                },
            )
        self._remove_integration(ctx, stack_id, [target for target, _, _ in plans])
        return parent_acceptance

    def _remove_integration(
        self, ctx: RequestContext, stack_id: str, targets: list[IntegrationTarget]
    ) -> None:
        for target in targets:
            path = self.app.stack_integration.worktree_path(stack_id, target.repository_name)
            with self.db.session() as session:
                row_ids = list(
                    session.scalars(
                        select(ManagedWorktree.id).where(
                            ManagedWorktree.path == str(path), ManagedWorktree.status == WorktreeStatus.ACTIVE
                        )
                    )
                )
            for row_id in row_ids:
                self.app.worktrees.remove(ctx, row_id, force=True)
            Git(target.source_path).run("branch", "-D", target.integration_branch, check=False)
            with self.db.session(write=True) as session:
                stack = session.get(Stack, stack_id)
                assert stack is not None
                record_event(
                    session,
                    ctx,
                    event_type="stack.integration_removed",
                    entity_type="stack",
                    entity_id=stack.id,
                    workspace_id=stack.workspace_id,
                    data={"repository": target.repository_name, "branch": target.integration_branch},
                )

    # ------------------------------------------------------------------ parent rejection

    @staticmethod
    def record_parent_rejected(session: Session, ctx: RequestContext, parent: Task, reason: str) -> None:
        """A rejected parent leaves its stack as it is; the rejection is recorded on the stack."""
        stack = StackLander.active_stack_of_parent(session, parent.id)
        if stack is None:
            return
        record_event(
            session,
            ctx,
            event_type="stack.parent_rejected",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={"task_key": parent.key, "reason": reason},
        )
