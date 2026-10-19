"""Stacks: the integration unit for a parent task's child changes.

A stack records, per repository, the commit its integration branch starts from (the default
branch head at creation) and the name of that branch. Creating a stack does not touch Git beyond
resolving that commit; the integration worktree is prepared by later stages.
"""

from __future__ import annotations

import builtins
from dataclasses import dataclass
from itertools import combinations
from pathlib import Path
from typing import Any

from sqlalchemy import and_, func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, NotFoundError, ValidationFailed
from workgraph.core.events.log import query_events, record_event
from workgraph.core.models.enums import (
    INACTIVE_STACK_STATUSES,
    STACK_DEPENDENCY_SATISFIED_STATES,
    AttemptStatus,
    StackItemState,
)
from workgraph.core.models.schemas import EventOut, StackItemOut, StackOut, StackRepositoryOut
from workgraph.core.policies.stacks import REMOVABLE_STATES, check_stack_item_transition
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.integrations.git.cli import Git
from workgraph.jobs import queue as jobqueue
from workgraph.jobs.limits import stack_key
from workgraph.persistence.orm import (
    AgentAttempt,
    ChangeSet,
    ChangeSetItem,
    Event,
    Job,
    Repository,
    Stack,
    StackItem,
    StackRepository,
    Task,
    TaskDependency,
    TaskRepository,
)
from workgraph.security.permissions import Permission, require


@dataclass(frozen=True)
class StackBase:
    """The commit a stack child's attempt starts from in one repository, and why it was chosen."""

    commit: str
    info: dict[str, Any]


def stack_base(session: Session, task: Task, repo: Repository) -> StackBase | None:
    """Choose where a stack child's attempt starts in one repository.

    Returns None when the task is not an item of an active stack that covers the repository; the
    attempt then starts from the default branch as before. A conflicting or ejected child starts from
    the integration tip. A child that depends on siblings in the stack which changed the repository
    starts from the head of the latest such sibling's succeeded attempt, so stacked changes build on
    each other. Any other child starts from the stack's base commit, so independent children run side
    by side on the same footing.
    """
    row = session.execute(
        select(Stack.id, StackRepository.base_sha, StackRepository.integration_branch, StackItem.state)
        .join(StackItem, StackItem.stack_id == Stack.id)
        .join(
            StackRepository,
            and_(StackRepository.stack_id == Stack.id, StackRepository.repository_id == repo.id),
        )
        .where(StackItem.task_id == task.id, Stack.status.not_in(INACTIVE_STACK_STATUSES))
    ).first()
    if row is None:
        return None
    stack_id, base_sha, integration_branch, state = row
    if StackItemState(state) in (StackItemState.CONFLICT, StackItemState.EJECTED):
        # A child sent back for a merge conflict, or ejected by verification, starts from what is integrated.
        checkout = primary_checkout(repo)
        if checkout is None:
            raise ValidationFailed(f"repository {repo.name!r} has no available checkout")
        tip = Git(Path(checkout.path)).resolve_commit(integration_branch)
        return StackBase(
            tip,
            {
                "source": "integration_tip",
                "stack_id": stack_id,
                "stack_base_sha": base_sha,
                "integration_branch": integration_branch,
                "integration_tip": tip,
            },
        )
    siblings = session.scalars(
        select(Task)
        .join(TaskDependency, TaskDependency.depends_on_id == Task.id)
        .join(StackItem, and_(StackItem.task_id == Task.id, StackItem.stack_id == stack_id))
        .where(TaskDependency.task_id == task.id)
        .order_by(Task.number)
    )
    heads: list[tuple[Task, str]] = []
    for sibling in siblings:
        head = latest_head(session, sibling.id, repo.id)
        if head is not None:
            heads.append((sibling, head))
    if not heads:
        return StackBase(base_sha, {"source": "stack_base", "stack_id": stack_id, "stack_base_sha": base_sha})
    checkout = primary_checkout(repo)
    if checkout is None:
        raise ValidationFailed(f"repository {repo.name!r} has no available checkout")
    git = Git(Path(checkout.path))
    for (first, first_head), (second, second_head) in combinations(heads, 2):
        if not (git.is_ancestor(first_head, second_head) or git.is_ancestor(second_head, first_head)):
            raise ValidationFailed(
                f"{task.key} depends on {first.key} and {second.key}, whose changes to {repo.name!r} "
                "are not one line of history; add an explicit dependency between "
                f"{first.key} and {second.key} so that one builds on the other",
                details={"task": task.key, "repository": repo.name, "siblings": [first.key, second.key]},
            )

    def builds_on_others(entry: tuple[Task, str]) -> int:
        return sum(
            1
            for other, other_head in heads
            if other.id != entry[0].id and git.is_ancestor(other_head, entry[1])
        )

    # The latest sibling is the one whose head builds on the most other sibling heads.
    ordered = sorted(heads, key=builds_on_others, reverse=True)
    (latest, head), superseded = ordered[0], ordered[1:]
    return StackBase(
        head,
        {
            "source": "sibling_head",
            "stack_id": stack_id,
            "stack_base_sha": base_sha,
            "sibling": latest.key,
            "sibling_head": head,
            "superseded": [{"task": other.key, "head": other_head} for other, other_head in superseded],
        },
    )


def latest_head(session: Session, task_id: str, repository_id: str) -> str | None:
    """The head the task's latest succeeded attempt left in the repository, if it touched it."""
    return session.scalar(
        select(ChangeSetItem.head_sha)
        .join(ChangeSet, ChangeSet.id == ChangeSetItem.change_set_id)
        .join(AgentAttempt, AgentAttempt.id == ChangeSet.attempt_id)
        .where(
            ChangeSet.task_id == task_id,
            ChangeSetItem.repository_id == repository_id,
            ChangeSetItem.head_sha.is_not(None),
            AgentAttempt.status == str(AttemptStatus.SUCCEEDED),
        )
        .order_by(ChangeSet.created_at.desc(), AgentAttempt.number.desc())
        .limit(1)
    )


def active_item(session: Session, task_id: str) -> tuple[Stack, StackItem] | None:
    """The task's item in its parent's active stack, if it is one."""
    row = session.execute(
        select(Stack, StackItem)
        .join(StackItem, StackItem.stack_id == Stack.id)
        .where(StackItem.task_id == task_id, Stack.status.not_in(INACTIVE_STACK_STATUSES))
    ).first()
    return (row[0], row[1]) if row is not None else None


def latest_event_id(session: Session, stack: Stack) -> str:
    """The id of the newest event about a stack; it versions the stack's integration jobs."""
    event_id = session.scalar(
        select(Event.id).where(Event.entity_id == stack.id).order_by(Event.seq.desc()).limit(1)
    )
    return event_id or stack.id


def stack_out(session: Session, stack: Stack) -> StackOut:
    parent = session.get(Task, stack.parent_task_id)
    assert parent is not None
    repo_rows = session.execute(
        select(StackRepository, Repository.name)
        .join(Repository, Repository.id == StackRepository.repository_id)
        .where(StackRepository.stack_id == stack.id)
        .order_by(Repository.name)
    ).all()
    item_rows = session.execute(
        select(StackItem, Task)
        .join(Task, Task.id == StackItem.task_id)
        .where(StackItem.stack_id == stack.id)
        .order_by(StackItem.merge_order.is_(None), StackItem.merge_order, Task.number)
    ).all()
    return StackOut(
        id=stack.id,
        workspace_id=stack.workspace_id,
        parent_task_id=stack.parent_task_id,
        parent_key=parent.key,
        parent_title=parent.title,
        status=stack.status,
        repositories=[
            StackRepositoryOut(
                repository_id=row.repository_id,
                repository_name=name,
                base_sha=row.base_sha,
                integration_branch=row.integration_branch,
            )
            for row, name in repo_rows
        ],
        items=[item_out(item, task) for item, task in item_rows],
        created_at=stack.created_at,
        updated_at=stack.updated_at,
    )


def item_out(item: StackItem, task: Task) -> StackItemOut:
    return StackItemOut(
        id=item.id,
        stack_id=item.stack_id,
        task_id=item.task_id,
        task_key=task.key,
        task_title=task.title,
        state=item.state,
        merge_order=item.merge_order,
        detail=item.detail,
        created_at=item.created_at,
        updated_at=item.updated_at,
    )


class StackService(Service):
    # ------------------------------------------------------------------ internal helpers

    @staticmethod
    def _active(session: Session, parent_id: str) -> Stack | None:
        return session.scalar(
            select(Stack).where(
                Stack.parent_task_id == parent_id, Stack.status.not_in(INACTIVE_STACK_STATUSES)
            )
        )

    def _active_or_raise(self, session: Session, parent: Task) -> Stack:
        stack = self._active(session, parent.id)
        if stack is None:
            raise NotFoundError(f"{parent.key} has no active stack", details={"parent": parent.key})
        return stack

    @staticmethod
    def _latest(session: Session, parent: Task) -> Stack:
        stack = session.scalar(
            select(Stack).where(Stack.parent_task_id == parent.id).order_by(Stack.created_at.desc()).limit(1)
        )
        if stack is None:
            raise NotFoundError(f"{parent.key} has no stack", details={"parent": parent.key})
        return stack

    @staticmethod
    def _item(session: Session, stack: Stack, task: Task) -> StackItem:
        item = session.scalar(
            select(StackItem).where(StackItem.stack_id == stack.id, StackItem.task_id == task.id)
        )
        if item is None:
            raise NotFoundError(f"{task.key} is not in the stack", details={"task": task.key})
        return item

    def _with_tips(self, stack: StackOut) -> StackOut:
        """Attach each integration branch tip that its prepared worktree can report."""
        repositories = [
            repo.model_copy(
                update={
                    "integration_tip": self.app.stack_integration.integration_tip(
                        stack.id, repo.repository_name
                    )
                }
            )
            for repo in stack.repositories
        ]
        return stack.model_copy(update={"repositories": repositories})

    @staticmethod
    def _base_sha(repo: Repository) -> str:
        """The default branch head of the repository's primary checkout, read at creation time."""
        if not repo.default_branch:
            raise ValidationFailed(f"repository {repo.name!r} has no default branch")
        checkout = primary_checkout(repo)
        if checkout is None:
            raise ValidationFailed(f"repository {repo.name!r} has no available checkout")
        return Git(Path(checkout.path)).resolve_commit(repo.default_branch)

    # ------------------------------------------------------------------ mutations

    def create(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> StackOut:
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            children = list(
                session.scalars(
                    select(Task)
                    .where(Task.parent_id == parent.id, Task.archived_at.is_(None))
                    .order_by(Task.number)
                )
            )
            if not children:
                raise ValidationFailed(
                    f"{parent.key} has no child tasks to stack", details={"parent": parent.key}
                )
            if self._active(session, parent.id) is not None:
                raise ConflictError(
                    f"{parent.key} already has an active stack", details={"parent": parent.key}
                )
            repo_ids = set(
                session.scalars(
                    select(TaskRepository.repository_id).where(
                        TaskRepository.task_id.in_([parent.id, *(child.id for child in children)])
                    )
                )
            )
            repos = list(
                session.scalars(
                    select(Repository).where(Repository.id.in_(repo_ids)).order_by(Repository.name)
                )
            )
            if not repos:
                raise ValidationFailed(
                    f"{parent.key} and its children are not linked to any repository",
                    details={"parent": parent.key},
                )
            branch = f"wg/stack/{parent.key.lower()}"
            base_shas = {repo.name: self._base_sha(repo) for repo in repos}
            stack = Stack(workspace_id=ws_id, parent_task_id=parent.id)
            session.add(stack)
            session.flush()
            for repo in repos:
                session.add(
                    StackRepository(
                        stack_id=stack.id,
                        repository_id=repo.id,
                        base_sha=base_shas[repo.name],
                        integration_branch=branch,
                    )
                )
            for child in children:
                session.add(StackItem(stack_id=stack.id, task_id=child.id, state=str(StackItemState.PENDING)))
            session.flush()
            record_event(
                session,
                ctx,
                event_type="stack.created",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=ws_id,
                data={
                    "parent_key": parent.key,
                    "integration_branch": branch,
                    "base_shas": base_shas,
                    "tasks": [child.key for child in children],
                },
            )
            return stack_out(session, stack)

    def add_item(
        self, ctx: RequestContext, workspace: str | None, parent_ref: str, task_ref: str
    ) -> StackOut:
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._active_or_raise(session, parent)
            child = resolve.task(session, ws_id, task_ref)
            if child.parent_id != parent.id:
                raise ValidationFailed(
                    f"{child.key} is not a child of {parent.key}", details={"task": child.key}
                )
            if session.scalar(
                select(StackItem.id).where(StackItem.stack_id == stack.id, StackItem.task_id == child.id)
            ):
                raise ConflictError(f"{child.key} is already in the stack", details={"task": child.key})
            child_repo_ids = set(
                session.scalars(
                    select(TaskRepository.repository_id).where(TaskRepository.task_id == child.id)
                )
            )
            stack_repo_ids = set(
                session.scalars(
                    select(StackRepository.repository_id).where(StackRepository.stack_id == stack.id)
                )
            )
            outside = sorted(
                session.scalars(
                    select(Repository.name).where(Repository.id.in_(child_repo_ids - stack_repo_ids))
                )
            )
            if outside:
                raise ValidationFailed(
                    f"{child.key} links repositories that are not part of the stack",
                    details={"task": child.key, "repositories": outside},
                )
            session.add(StackItem(stack_id=stack.id, task_id=child.id, state=str(StackItemState.PENDING)))
            stack.updated_at = utcnow()
            session.flush()
            record_event(
                session,
                ctx,
                event_type="stack.item_added",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=ws_id,
                data={"task_key": child.key, "parent_key": parent.key},
            )
            return stack_out(session, stack)

    def remove_item(
        self, ctx: RequestContext, workspace: str | None, parent_ref: str, task_ref: str
    ) -> StackOut:
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._active_or_raise(session, parent)
            child = resolve.task(session, ws_id, task_ref)
            item = self._item(session, stack, child)
            state = StackItemState(item.state)
            if state not in REMOVABLE_STATES:
                raise ConflictError(
                    f"{child.key} is {state} on the integration branch and cannot leave the stack",
                    details={"task": child.key, "state": state},
                )
            session.delete(item)
            stack.updated_at = utcnow()
            session.flush()
            event = record_event(
                session,
                ctx,
                event_type="stack.item_removed",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=ws_id,
                data={"task_key": child.key, "parent_key": parent.key, "state": state},
            )
            if state in STACK_DEPENDENCY_SATISFIED_STATES:
                # Dependents that relied on this sibling inside the stack must wait for acceptance again.
                self.app.tasks._refresh_dependents(session, ctx.caused_by(event.id), child.id)
            return stack_out(session, stack)

    def set_item_state(
        self,
        ctx: RequestContext,
        workspace: str | None,
        parent_ref: str,
        task_ref: str,
        state: StackItemState,
        detail: str | None = None,
    ) -> StackOut:
        require(ctx, Permission.STACK_WRITE)
        target = StackItemState(state)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._active_or_raise(session, parent)
            child = resolve.task(session, ws_id, task_ref)
            item = self._item(session, stack, child)
            self.transition_item(session, ctx, stack, parent, child, item, target, detail)
            return stack_out(session, stack)

    def readmit_item(
        self, ctx: RequestContext, workspace: str | None, parent_ref: str, task_ref: str
    ) -> StackOut:
        """Queue an ejected (or re-added, pending) child for integration again without a new fast gate.

        For an operator who judged the ejection to be caused by something outside the change, such as
        a polluted build or a timeout under load. The next integration merges it and full verification
        judges it again.
        """
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._active_or_raise(session, parent)
            child = resolve.task(session, ws_id, task_ref)
            item = self._item(session, stack, child)
            state = StackItemState(item.state)
            if state not in (StackItemState.EJECTED, StackItemState.PENDING):
                raise ConflictError(
                    f"{child.key} is {state}; only an ejected or pending child can be readmitted",
                    details={"task": child.key, "state": str(state)},
                )
            self.transition_item(session, ctx, stack, parent, child, item, StackItemState.READY, item.detail)
            return stack_out(session, stack)

    def mark_ready(self, ctx: RequestContext, task_id: str) -> None:
        """Mark a stack child ready after its fast tier passed; a task outside an active stack is left alone."""
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            found = active_item(session, task_id)
            if found is None:
                return
            stack, item = found
            child = session.get(Task, task_id)
            parent = session.get(Task, stack.parent_task_id)
            assert child is not None and parent is not None
            self.transition_item(session, ctx, stack, parent, child, item, StackItemState.READY, item.detail)

    def gate(self, task_id: str) -> str | None:
        """How a finished attempt is verified: None outside a stack, ``fast`` or ``pass`` inside one.

        ``pass`` means the stack child has no fast checks, so it becomes ready without running any.
        """
        from workgraph.verification.configs import applicable_checks, checks_for_tier

        with self.db.session() as session:
            if active_item(session, task_id) is None:
                return None
            task = session.get(Task, task_id)
            assert task is not None
            return "fast" if checks_for_tier(applicable_checks(session, task), "fast") else "pass"

    def integrate(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> dict[str, Any]:
        """Queue an integration run for each repository of the parent's active stack."""
        require(ctx, Permission.STACK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._active_or_raise(session, parent)
            version = latest_event_id(session, stack)
            jobs = self.enqueue_integration(session, ctx, stack, version)
            names = dict(
                session.execute(
                    select(StackRepository.repository_id, Repository.name)
                    .join(Repository, Repository.id == StackRepository.repository_id)
                    .where(StackRepository.stack_id == stack.id)
                ).all()
            )
            return {
                "stack": parent.key,
                "jobs": [{"repository": names[repo_id], "job_id": job.id} for repo_id, job in jobs],
            }

    def enqueue_integration(
        self,
        session: Session,
        ctx: RequestContext,
        stack: Stack,
        version: str,
        *,
        exclude_repository_id: str | None = None,
    ) -> builtins.list[tuple[str, Job]]:
        """Queue one ``stack_integrate`` job per stack repository, holding that stack's key for the repository.

        The key names the stack state version (an event id), so repeated requests for an unchanged
        stack collapse into one job while every new state change gets its own. Only this stack's
        key is held, so other stacks on the same repository integrate and verify independently.
        """
        repo_ids = [
            repo_id
            for repo_id in session.scalars(
                select(StackRepository.repository_id)
                .where(StackRepository.stack_id == stack.id)
                .order_by(StackRepository.repository_id)
            )
            if repo_id != exclude_repository_id
        ]
        queued = []
        for repo_id in repo_ids:
            job = jobqueue.enqueue(
                session,
                "stack_integrate",
                {"stack_id": stack.id, "repository_id": repo_id},
                workspace_id=stack.workspace_id,
                idempotency_key=f"stack-integrate:{stack.id}:{repo_id}:{version}",
                resource_keys=[stack_key(stack.id, repo_id)],
                causation_id=version,
                created_by=ctx.actor,
            )
            queued.append((repo_id, job))
        return queued

    def transition_item(
        self,
        session: Session,
        ctx: RequestContext,
        stack: Stack,
        parent: Task,
        child: Task,
        item: StackItem,
        target: StackItemState,
        detail: str | None,
    ) -> None:
        current = StackItemState(item.state)
        if current == target:
            return
        check_stack_item_transition(current, target)
        item.state = str(target)
        item.detail = detail
        if target == StackItemState.MERGED and item.merge_order is None:
            # Merge order is the order in which children first reached the integration branch.
            # Rebuilds replay children in this recorded order.
            last = session.scalar(
                select(func.max(StackItem.merge_order)).where(StackItem.stack_id == stack.id)
            )
            item.merge_order = (last or 0) + 1
        stack.updated_at = utcnow()
        session.flush()
        event = record_event(
            session,
            ctx,
            event_type="stack.item_state_changed",
            entity_type="stack",
            entity_id=stack.id,
            workspace_id=stack.workspace_id,
            data={
                "task_key": child.key,
                "parent_key": parent.key,
                "from": current,
                "to": target,
                "detail": detail,
                "merge_order": item.merge_order,
            },
        )
        # Entering or leaving integration-ready changes whether dependents on this sibling are satisfied.
        if (current in STACK_DEPENDENCY_SATISFIED_STATES) != (target in STACK_DEPENDENCY_SATISFIED_STATES):
            self.app.tasks._refresh_dependents(session, ctx.caused_by(event.id), child.id)
        if target == StackItemState.READY:
            self.enqueue_integration(session, ctx, stack, event.id)

    # ------------------------------------------------------------------ reads

    def get(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> StackOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            return self._with_tips(stack_out(session, self._latest(session, parent)))

    def history(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> builtins.list[EventOut]:
        """The latest stack's events in order, with the verification runs its events name.

        Stack events carry the verification run they came from (``run_id``, or ``verification_run_id``
        for ejections), so the runs are found through those ids rather than by scanning all events.
        """
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            stack = self._latest(session, parent)
            events = query_events(session, entity_type="stack", entity_ids=[stack.id], limit=10000)
            run_ids: set[str] = set()
            for event in events:
                for key in ("run_id", "verification_run_id"):
                    value = event.data.get(key)
                    if isinstance(value, str):
                        run_ids.add(value)
            runs = (
                query_events(session, entity_type="verification_run", entity_ids=sorted(run_ids), limit=10000)
                if run_ids
                else []
            )
            merged = sorted([*events, *runs], key=lambda event: event.seq)
            return [EventOut.model_validate(event.to_dict()) for event in merged]

    def list(
        self, ctx: RequestContext, workspace: str | None, *, include_inactive: bool = False
    ) -> builtins.list[StackOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(Stack).where(Stack.workspace_id == ws_id).order_by(Stack.created_at.desc())
            if not include_inactive:
                stmt = stmt.where(Stack.status.not_in(INACTIVE_STACK_STATUSES))
            return [self._with_tips(stack_out(session, stack)) for stack in session.scalars(stmt)]

    def for_task(
        self, ctx: RequestContext, workspace: str | None, task_ref: str
    ) -> builtins.list[StackItemOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            child = resolve.task(session, ws_id, task_ref)
            rows = session.execute(
                select(StackItem, Task)
                .join(Stack, Stack.id == StackItem.stack_id)
                .join(Task, Task.id == StackItem.task_id)
                .where(StackItem.task_id == child.id)
                .order_by(Stack.created_at.desc())
            ).all()
            return [item_out(item, task) for item, task in rows]

    def containing(
        self, ctx: RequestContext, workspace: str | None, task_ref: str
    ) -> builtins.list[StackOut]:
        """Every stack the task is an item of, newest first (inactive ones included)."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            child = resolve.task(session, ws_id, task_ref)
            stacks = session.scalars(
                select(Stack)
                .join(StackItem, StackItem.stack_id == Stack.id)
                .where(StackItem.task_id == child.id)
                .order_by(Stack.created_at.desc())
            ).all()
            return [stack_out(session, stack) for stack in stacks]
