"""Stacks: the integration unit for a parent task's child changes.

A stack records, per repository, the commit its integration branch starts from (the default
branch head at creation) and the name of that branch. Creating a stack does not touch Git beyond
resolving that commit; the integration worktree is prepared by later stages.
"""

from __future__ import annotations

import builtins
from pathlib import Path

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import StackItemState, StackStatus
from workgraph.core.models.schemas import StackItemOut, StackOut, StackRepositoryOut
from workgraph.core.policies.stacks import REMOVABLE_STATES, check_stack_item_transition
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.integrations.git.cli import Git
from workgraph.persistence.orm import (
    Repository,
    Stack,
    StackItem,
    StackRepository,
    Task,
    TaskRepository,
)
from workgraph.security.permissions import Permission, require

# Stacks in these states are history; a parent may start a new stack once its last one is inactive.
INACTIVE_STACK_STATUSES = (str(StackStatus.LANDED), str(StackStatus.ABANDONED))


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
            record_event(
                session,
                ctx,
                event_type="stack.item_removed",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=ws_id,
                data={"task_key": child.key, "parent_key": parent.key, "state": state},
            )
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
            current = StackItemState(item.state)
            if current == target:
                return stack_out(session, stack)
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
            record_event(
                session,
                ctx,
                event_type="stack.item_state_changed",
                entity_type="stack",
                entity_id=stack.id,
                workspace_id=ws_id,
                data={
                    "task_key": child.key,
                    "parent_key": parent.key,
                    "from": current,
                    "to": target,
                    "detail": detail,
                    "merge_order": item.merge_order,
                },
            )
            return stack_out(session, stack)

    # ------------------------------------------------------------------ reads

    def get(self, ctx: RequestContext, workspace: str | None, parent_ref: str) -> StackOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            parent = resolve.task(session, ws_id, parent_ref)
            return stack_out(session, self._latest(session, parent))

    def list(
        self, ctx: RequestContext, workspace: str | None, *, include_inactive: bool = False
    ) -> builtins.list[StackOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(Stack).where(Stack.workspace_id == ws_id).order_by(Stack.created_at.desc())
            if not include_inactive:
                stmt = stmt.where(Stack.status.not_in(INACTIVE_STACK_STATUSES))
            return [stack_out(session, stack) for stack in session.scalars(stmt)]

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
