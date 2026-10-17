"""Stack integration: merging ready stack children onto each repository's integration branch.

Every stack repository has one managed worktree on its integration branch, created at the stack's
base commit. The ``stack_integrate`` job merges ready items into it one at a time, in dependency,
priority and readiness order. A merge that conflicts is aborted: the item moves to ``conflict``, its
task returns to ready with the conflicting paths and the integration tip in a comment, and the next
attempt starts from that tip (see ``core.services.stacks.stack_base``).

Per-item progress lives in ``stack_items.detail`` as JSON: ``merges`` maps each repository name to
the change head that was merged and the merge commit, and ``conflicts`` holds the open conflicts.
Once the queue settles, ``stack_verify`` jobs run the full tier on the integration tip (see
``integrations.git.stack_verification``).
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import GitError, NotFoundError
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    INACTIVE_STACK_STATUSES,
    PRIORITY_RANK,
    StackItemState,
    StackStatus,
    TaskPriority,
    TaskStatus,
    WorktreeStatus,
)
from workgraph.core.models.schemas import CommentCreate
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.core.services.stacks import latest_event_id, latest_head
from workgraph.integrations.git.cli import Git
from workgraph.persistence.orm import (
    ManagedWorktree,
    Repository,
    Stack,
    StackItem,
    StackRepository,
    Task,
    TaskDependency,
)

# Integration merges are commits made by Workgraph itself, not by the agent or the user.
INTEGRATION_IDENTITY = {
    "GIT_AUTHOR_NAME": "Workgraph Stack Integration",
    "GIT_AUTHOR_EMAIL": "integration@workgraph.local",
    "GIT_COMMITTER_NAME": "Workgraph Stack Integration",
    "GIT_COMMITTER_EMAIL": "integration@workgraph.local",
}

# A conflicting child returns to its agent from either state; a verified child is still on the branch.
RETURNABLE_TASK_STATUSES = (TaskStatus.AWAITING_VERIFICATION, TaskStatus.AWAITING_REVIEW)


@dataclass(frozen=True)
class IntegrationTarget:
    stack_id: str
    workspace_id: str
    repository_id: str
    repository_name: str
    integration_branch: str
    base_sha: str
    source_path: str
    checkout_id: str


@dataclass(frozen=True)
class MergeOutcome:
    """The result of merging one head into an integration worktree.

    ``conflict_paths`` is None when the merge was made (or was already contained in the branch); a
    conflicting merge is aborted, so ``merge_commit`` is then the tip the branch still points at.
    """

    merge_commit: str
    conflict_paths: list[str] | None = None
    git_error: str = ""
    already_integrated: bool = False


def load_detail(text: str | None) -> dict[str, Any]:
    """An item's detail. Free text (set by an operator) carries no merges, so it reads as empty."""
    try:
        parsed = json.loads(text) if text else {}
    except json.JSONDecodeError:
        parsed = {}
    if not isinstance(parsed, dict):
        parsed = {}
    return {
        **parsed,
        "merges": dict(parsed.get("merges", {})),
        "conflicts": dict(parsed.get("conflicts", {})),
    }


def dump_detail(detail: dict[str, Any]) -> str:
    return json.dumps(detail, sort_keys=True)


def directory_name(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", name)


def merge_head(git: Git, head: str, message: str) -> MergeOutcome:
    """Merge ``head`` into the worktree's branch as Workgraph's integration identity."""
    tip_before = git.head()
    assert tip_before is not None
    if git.is_ancestor(head, tip_before):
        return MergeOutcome(merge_commit=tip_before, already_integrated=True)
    result = git.run(
        "merge", "--no-ff", "--no-edit", "-m", message, head, check=False, env=INTEGRATION_IDENTITY
    )
    if result.returncode == 0:
        merged = git.head()
        assert merged is not None
        return MergeOutcome(merge_commit=merged)
    unmerged = git.out("diff", "--name-only", "--diff-filter=U", check=False)
    conflict_paths = [line for line in unmerged.splitlines() if line]
    git_error = result.stderr.strip()[:500]
    git.run("merge", "--abort", check=False)
    return MergeOutcome(merge_commit=tip_before, conflict_paths=conflict_paths, git_error=git_error)


class StackIntegrator(Service):
    # ------------------------------------------------------------------ worktree

    def worktree_path(self, stack_id: str, repository_name: str) -> Path:
        """Where a stack's integration worktree for one repository lives."""
        return self.app.paths.worktrees_dir / "stacks" / stack_id / directory_name(repository_name)

    def ensure_worktree(self, ctx: RequestContext, target: IntegrationTarget) -> Path:
        """The integration worktree for a stack repository, created on first use."""
        path = self.worktree_path(target.stack_id, target.repository_name)
        with self.db.session() as session:
            row = session.scalar(select(ManagedWorktree).where(ManagedWorktree.path == str(path)))
            if (
                row is not None
                and row.status == WorktreeStatus.ACTIVE
                and path.exists()
                and Git(path).is_repository()
            ):
                return path
        source = Git(target.source_path)
        source.run("worktree", "prune", check=False)
        path.parent.mkdir(parents=True, exist_ok=True)
        branch = target.integration_branch
        if (
            source.run("rev-parse", "--verify", "--quiet", f"refs/heads/{branch}", check=False).returncode
            == 0
        ):
            source.run("worktree", "add", str(path), branch)
        else:
            source.run("worktree", "add", "-b", branch, str(path), target.base_sha)
        with self.db.session(write=True) as session:
            row = session.scalar(select(ManagedWorktree).where(ManagedWorktree.path == str(path)))
            if row is None:
                row = ManagedWorktree(
                    workspace_id=target.workspace_id,
                    repository_id=target.repository_id,
                    source_checkout_id=target.checkout_id,
                    attempt_id=None,
                    path=str(path),
                    branch=branch,
                    base_sha=target.base_sha,
                )
                session.add(row)
            row.status = WorktreeStatus.ACTIVE
            session.flush()
            record_event(
                session,
                ctx,
                event_type="worktree.created",
                entity_type="worktree",
                entity_id=row.id,
                workspace_id=target.workspace_id,
                data={
                    "purpose": "stack_integration",
                    "stack_id": target.stack_id,
                    "repository": target.repository_name,
                    "path": str(path),
                    "branch": branch,
                    "base_sha": target.base_sha,
                },
            )
        return path

    # ------------------------------------------------------------------ job

    def integrate(self, ctx: RequestContext, stack_id: str, repository_id: str) -> dict[str, Any]:
        """Merge every due ready item, in order, into one repository's integration branch."""
        target = self.target(stack_id, repository_id)
        if target is None:
            return {"status": "inactive", "merged": [], "skipped": [], "conflicts": []}
        git = Git(self.ensure_worktree(ctx, target))
        outcome: dict[str, list[str]] = {"merged": [], "skipped": [], "conflicts": []}
        processed: set[str] = set()
        progressed = False
        while (item_id := self._next_due(target, processed)) is not None:
            processed.add(item_id)
            kind, task_key = self._integrate_item(ctx, target, git, item_id)
            if kind == "conflict":
                outcome["conflicts"].append(task_key)
            elif kind == "skipped":
                outcome["skipped"].append(task_key)
            elif kind == "merged":
                outcome["merged"].append(task_key)
                progressed = True
        if progressed:
            # A merge can unblock dependents that wait on it in another repository.
            with self.db.session(write=True) as session:
                stack = session.get(Stack, stack_id)
                if stack is not None and stack.status not in INACTIVE_STACK_STATUSES:
                    self.app.stacks.enqueue_integration(
                        session,
                        ctx,
                        stack,
                        latest_event_id(session, stack),
                        exclude_repository_id=repository_id,
                    )
        self.app.stack_verification.enqueue_if_settled(ctx, stack_id)
        return {"status": "ran", "repository": target.repository_name, **outcome}

    def target(self, stack_id: str, repository_id: str) -> IntegrationTarget | None:
        with self.db.session() as session:
            row = session.execute(
                select(Stack, StackRepository, Repository)
                .join(StackRepository, StackRepository.stack_id == Stack.id)
                .join(Repository, Repository.id == StackRepository.repository_id)
                .where(Stack.id == stack_id, StackRepository.repository_id == repository_id)
            ).first()
            if row is None:
                raise NotFoundError(f"stack {stack_id} has no repository {repository_id}")
            stack, link, repo = row
            if stack.status in INACTIVE_STACK_STATUSES:
                return None
            checkout = primary_checkout(repo)
            if checkout is None:
                raise GitError(f"repository {repo.name} has no available checkout")
            return IntegrationTarget(
                stack_id=stack.id,
                workspace_id=stack.workspace_id,
                repository_id=repo.id,
                repository_name=repo.name,
                integration_branch=link.integration_branch,
                base_sha=link.base_sha,
                source_path=checkout.path,
                checkout_id=checkout.id,
            )

    def _next_due(self, target: IntegrationTarget, processed: set[str]) -> str | None:
        """The next ready item whose in-stack dependencies are merged, by depth, priority, then readiness."""
        with self.db.session() as session:
            rows = session.execute(
                select(StackItem, Task)
                .join(Task, Task.id == StackItem.task_id)
                .where(StackItem.stack_id == target.stack_id)
            ).all()
            states = {item.task_id: StackItemState(item.state) for item, _ in rows}
            edges = session.execute(
                select(TaskDependency.task_id, TaskDependency.depends_on_id).where(
                    TaskDependency.task_id.in_(list(states)), TaskDependency.depends_on_id.in_(list(states))
                )
            ).all()
            prerequisites: dict[str, list[str]] = {}
            for dependent, prerequisite in edges:
                prerequisites.setdefault(dependent, []).append(prerequisite)
            depths: dict[str, int] = {}

            def depth(task_id: str) -> int:
                if task_id not in depths:
                    deps = prerequisites.get(task_id, [])
                    depths[task_id] = 1 + max(depth(p) for p in deps) if deps else 0
                return depths[task_id]

            due = []
            for item, task in rows:
                if item.id in processed or StackItemState(item.state) != StackItemState.READY:
                    continue
                if any(
                    states[p] not in (StackItemState.MERGED, StackItemState.VERIFIED)
                    for p in prerequisites.get(task.id, [])
                ):
                    continue
                rank = PRIORITY_RANK[TaskPriority(task.priority)]
                due.append(((depth(task.id), rank, item.updated_at, task.number), item.id))
            return min(due)[1] if due else None

    # ------------------------------------------------------------------ one item

    def _integrate_item(
        self, ctx: RequestContext, target: IntegrationTarget, git: Git, item_id: str
    ) -> tuple[str, str]:
        """Merge one item into the integration branch; returns (outcome, task key)."""
        with self.db.session(write=True) as session:
            stack, item, task, parent = self.load_item(session, item_id)
            heads = self.heads(session, stack, task)
            head = heads.get(target.repository_id)
            detail = load_detail(item.detail)
            if not heads:
                # Nothing changed anywhere, so there is nothing to merge and the item is integrated as is.
                self.app.stacks.transition_item(
                    session, ctx, stack, parent, task, item, StackItemState.MERGED, item.detail
                )
                return "merged", task.key
            if head is None:
                record_event(
                    session,
                    ctx,
                    event_type="stack.integration_skipped",
                    entity_type="stack",
                    entity_id=stack.id,
                    workspace_id=stack.workspace_id,
                    data={
                        "task_key": task.key,
                        "repository": target.repository_name,
                        "reason": "no change in this repository",
                    },
                )
                return "skipped", task.key
            if detail["merges"].get(target.repository_name, {}).get("head") == head:
                return "noop", task.key
            self.start_integrating(session, ctx, stack)
            task_key, title = task.key, task.title
        assert head is not None

        message = f"Merge {task_key}: {title} into {target.integration_branch}"
        outcome = merge_head(git, head, message)

        with self.db.session(write=True) as session:
            stack, item, task, parent = self.load_item(session, item_id)
            detail = load_detail(item.detail)
            if outcome.conflict_paths is None:
                detail["merges"][target.repository_name] = {
                    "head": head,
                    "merge_commit": outcome.merge_commit,
                }
                detail["conflicts"].pop(target.repository_name, None)
                record_event(
                    session,
                    ctx,
                    event_type="stack.integration_merged",
                    entity_type="stack",
                    entity_id=stack.id,
                    workspace_id=stack.workspace_id,
                    data={
                        "task_key": task.key,
                        "repository": target.repository_name,
                        "branch": target.integration_branch,
                        "head": head,
                        "merge_commit": outcome.merge_commit,
                        "already_integrated": outcome.already_integrated,
                    },
                )
                if self.fully_merged(session, stack, task, detail):
                    self.app.stacks.transition_item(
                        session, ctx, stack, parent, task, item, StackItemState.MERGED, dump_detail(detail)
                    )
                else:
                    item.detail = dump_detail(detail)
                    stack.updated_at = utcnow()
                return "merged", task.key

            self.record_conflict(
                session,
                ctx,
                stack,
                parent,
                task,
                item,
                repository=target.repository_name,
                branch=target.integration_branch,
                head=head,
                outcome=outcome,
                detail=detail,
            )
            return "conflict", task.key

    def record_conflict(
        self,
        session: Session,
        ctx: RequestContext,
        stack: Stack,
        parent: Task,
        task: Task,
        item: StackItem,
        *,
        repository: str,
        branch: str,
        head: str,
        outcome: MergeOutcome,
        detail: dict[str, Any],
    ) -> None:
        """Send an item back to its agent after a merge into one repository conflicted.

        The merge was aborted, so the branch did not move. The item moves to ``conflict`` (from ready,
        merged or verified) and its task returns to ready with a comment naming the conflict.
        """
        paths = outcome.conflict_paths or []
        conflict = {
            "repository": repository,
            "head": head,
            "paths": paths,
            "integration_tip": outcome.merge_commit,
            "instruction": (
                f"rebase this change onto the integration tip {outcome.merge_commit} of "
                f"{branch} and resolve the conflicts there"
            ),
            "git_error": "" if paths else outcome.git_error,
        }
        detail["conflicts"][repository] = conflict
        record_event(
            session,
            ctx,
            event_type="stack.integration_conflict",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={"task_key": task.key, **conflict},
        )
        item.detail = dump_detail(detail)
        self.app.stacks.transition_item(
            session, ctx, stack, parent, task, item, StackItemState.CONFLICT, dump_detail(detail)
        )
        if TaskStatus(task.status) in RETURNABLE_TASK_STATUSES:
            self.app.tasks._transition(
                session, ctx, task, TaskStatus.READY, reason=f"integration conflict in {repository}"
            )
        listed = ", ".join(paths) if paths else "none reported"
        self.app.tasks._comment(
            session,
            ctx,
            task,
            CommentCreate(
                body=(
                    f"Merge conflict integrating {task.key} into {branch} ({repository}). "
                    f"Conflicting paths: {listed}. Integration tip: {outcome.merge_commit}. "
                    f"Next step: {conflict['instruction']}."
                ),
                data={"stack_item_conflict": conflict},
            ),
        )

    # ------------------------------------------------------------------ helpers

    @staticmethod
    def load_item(session: Session, item_id: str) -> tuple[Stack, StackItem, Task, Task]:
        item = session.get(StackItem, item_id)
        assert item is not None
        stack = session.get(Stack, item.stack_id)
        task = session.get(Task, item.task_id)
        assert stack is not None and task is not None
        parent = session.get(Task, stack.parent_task_id)
        assert parent is not None
        return stack, item, task, parent

    @staticmethod
    def heads(session: Session, stack: Stack, task: Task) -> dict[str, str]:
        """The head the task left in each stack repository it changed."""
        heads: dict[str, str] = {}
        for repo_id in session.scalars(
            select(StackRepository.repository_id).where(StackRepository.stack_id == stack.id)
        ):
            head = latest_head(session, task.id, repo_id)
            if head is not None:
                heads[repo_id] = head
        return heads

    @staticmethod
    def fully_merged(session: Session, stack: Stack, task: Task, detail: dict[str, Any]) -> bool:
        """Whether every repository the item changed has merged its current head."""
        names = dict(
            session.execute(
                select(StackRepository.repository_id, Repository.name)
                .join(Repository, Repository.id == StackRepository.repository_id)
                .where(StackRepository.stack_id == stack.id)
            ).all()
        )
        heads = StackIntegrator.heads(session, stack, task)
        return all(
            detail["merges"].get(names[repo_id], {}).get("head") == head for repo_id, head in heads.items()
        )

    @staticmethod
    def start_integrating(session: Session, ctx: RequestContext, stack: Stack) -> None:
        """Move a stack into ``integrating`` when its branch is about to change.

        A verified stack is no longer verified once a new child lands on its branch.
        """
        if stack.status not in (StackStatus.OPEN, StackStatus.VERIFIED):
            return
        previous = stack.status
        stack.status = StackStatus.INTEGRATING
        stack.updated_at = utcnow()
        session.flush()
        record_event(
            session,
            ctx,
            event_type="stack.status_changed",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={"from": previous, "to": stack.status, "reason": "integration started"},
        )
