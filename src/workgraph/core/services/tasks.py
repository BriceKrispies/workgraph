"""Task management: CRUD, lifecycle, dependencies, relations, claims and graphs.

Every mutation:

1. checks the actor's permission,
2. validates input and lifecycle rules,
3. updates current-state tables, and
4. appends a domain event in the **same** transaction.
"""

from __future__ import annotations

import builtins
import hashlib
import re
from collections import defaultdict, deque
from collections.abc import Collection, Iterable
from datetime import timedelta
from typing import Any

from sqlalchemy import Select, and_, case, delete, func, or_, select, text
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConcurrencyError,
    ConflictError,
    DependencyCycleError,
    InvalidTransitionError,
    NotFoundError,
    PermissionDenied,
    ResourceBusyError,
    ValidationFailed,
)
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    DEPENDENCY_SATISFIED_STATUSES,
    INACTIVE_STACK_STATUSES,
    PRIORITY_RANK,
    STACK_DEPENDENCY_SATISFIED_STATES,
    AttemptStatus,
    CommentKind,
    StackItemState,
    TaskPriority,
    TaskStatus,
)
from workgraph.core.models.schemas import (
    BulkResult,
    BulkUpdate,
    ClaimRequest,
    CommentCreate,
    CommentOut,
    DependencyState,
    GraphEdge,
    GraphNode,
    Page,
    ProgressReport,
    RelationCreate,
    RelationOut,
    ResultReport,
    TaskBrief,
    TaskCreate,
    TaskDetail,
    TaskFilter,
    TaskGraph,
    TaskOut,
    TaskRollup,
    TaskUpdate,
)
from workgraph.core.policies.lifecycle import REQUIRES_DEPENDENCIES, check_task_transition
from workgraph.core.policies.trackers import tracker_target_status
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.integrations.trackers.types import SYNC_ACTOR
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    AgentAttempt,
    ExternalItem,
    Project,
    Repository,
    Stack,
    StackItem,
    Task,
    TaskComment,
    TaskDependency,
    TaskFile,
    TaskLink,
    TaskProject,
    TaskRelation,
    TaskRepository,
    TaskTag,
    Workspace,
)
from workgraph.search.indexer import index_task, to_fts_query
from workgraph.security.permissions import Permission, require

BLOCKED_BY_DEPENDENCIES = "dependencies"
TRACKER_KEY_RE = re.compile(r"[A-Z][A-Z0-9]{0,9}-\d+")

TRACKER_OWNED_FIELDS = frozenset(
    {"title", "description", "acceptance_criteria", "priority", "assignee", "parent", "archived", "status"}
)
"""Fields a tracker owns on a tracked task. Only the sync may change them.

``status`` is guarded only for changes that accept, cancel or reopen the task (see ``_update_in_session``).
"""


def content_hash(title: str, description: str) -> str:
    normalized = " ".join(title.split()).lower() + "\n" + " ".join(description.split()).lower()
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


# --------------------------------------------------------------------------- read helpers


def _check_tracker_key(session: Session, ctx: RequestContext, ws: Workspace, key: str) -> None:
    """Only the tracker sync may name a task after its tracker issue; the key must not shadow a local one."""
    if ctx.actor != SYNC_ACTOR:
        raise PermissionDenied(f"only {SYNC_ACTOR} may set a task key")
    if TRACKER_KEY_RE.fullmatch(key) is None:
        raise ValidationFailed(f"tracker key {key!r} must look like ABC-123")
    prefix = key.split("-", 1)[0]
    if prefix == ws.key_prefix:
        raise ValidationFailed(f"tracker key prefix {prefix} is the workspace's own prefix")
    if session.scalar(select(Task.id).where(Task.workspace_id == ws.id, Task.key == key)) is not None:
        raise ConflictError(f"task {key} already exists in this workspace")


def _file_dicts(session: Session, task_id: str) -> builtins.list[dict[str, Any]]:
    rows = session.execute(
        select(TaskFile.path, TaskFile.anchor, TaskFile.note)
        .where(TaskFile.task_id == task_id)
        .order_by(TaskFile.position)
    ).all()
    return [{"path": path, "anchor": anchor, "note": note} for path, anchor, note in rows]


def _set_files(session: Session, task_id: str, files: Iterable[dict[str, Any]]) -> None:
    session.execute(delete(TaskFile).where(TaskFile.task_id == task_id))
    for position, item in enumerate(files):
        session.add(
            TaskFile(
                task_id=task_id,
                position=position,
                path=item["path"],
                anchor=item["anchor"],
                note=item["note"],
            )
        )


def task_out(session: Session, task: Task, cls: type[TaskOut] = TaskOut) -> Any:
    project_ids = session.scalars(select(TaskProject.project_id).where(TaskProject.task_id == task.id)).all()
    repo_ids = session.scalars(
        select(TaskRepository.repository_id).where(TaskRepository.task_id == task.id)
    ).all()
    child_ids = session.scalars(select(Task.id).where(Task.parent_id == task.id).order_by(Task.number)).all()
    depends_on = session.scalars(
        select(TaskDependency.depends_on_id).where(TaskDependency.task_id == task.id)
    ).all()
    blocks = session.scalars(
        select(TaskDependency.task_id).where(TaskDependency.depends_on_id == task.id)
    ).all()
    data = {c.key: getattr(task, c.key) for c in Task.__mapper__.column_attrs}
    data["metadata"] = data.pop("metadata_")
    data["tags"] = [t.tag for t in task.tags]
    data["files"] = _file_dicts(session, task.id)
    data["project_ids"] = sorted(project_ids)
    data["repository_ids"] = sorted(repo_ids)
    data["child_ids"] = list(child_ids)
    data["depends_on"] = sorted(depends_on)
    data["blocks"] = sorted(blocks)
    data["links"] = session.scalars(
        select(TaskLink).where(TaskLink.task_id == task.id).order_by(TaskLink.kind, TaskLink.url)
    ).all()
    return cls.model_validate(data)


def is_tracked(session: Session, task_id: str) -> ExternalItem | None:
    """The tracker issue a task mirrors, or None for a local task."""
    return session.scalar(select(ExternalItem).where(ExternalItem.task_id == task_id))


def satisfied_dependencies(session: Session, edges: Collection[tuple[str, str]]) -> set[tuple[str, str]]:
    """Which ``(task_id, depends_on_id)`` edges are satisfied.

    A dependency is satisfied once its task is accepted. Inside a stack, a dependency on a sibling
    that is an item of the same active stack is satisfied as soon as that item is integration-ready
    (ready, merged or verified). Every dependency check goes through here.
    """
    if not edges:
        return set()
    statuses = dict(
        session.execute(select(Task.id, Task.status).where(Task.id.in_({d for _, d in edges}))).all()
    )
    satisfied = {(t, d) for t, d in edges if statuses.get(d) in DEPENDENCY_SATISFIED_STATUSES}
    pending = {(t, d) for t, d in edges if (t, d) not in satisfied}
    if not pending:
        return satisfied
    members: dict[str, dict[str, str]] = defaultdict(dict)  # stack id -> task id -> item state
    for stack_id, task_id, state in session.execute(
        select(StackItem.stack_id, StackItem.task_id, StackItem.state)
        .join(Stack, Stack.id == StackItem.stack_id)
        .where(
            StackItem.task_id.in_({t for t, _ in pending} | {d for _, d in pending}),
            Stack.status.not_in(INACTIVE_STACK_STATUSES),
        )
    ):
        members[stack_id][task_id] = state
    for task_id, depends_on_id in pending:
        for items in members.values():
            if task_id in items and depends_on_id in items:
                if StackItemState(items[depends_on_id]) in STACK_DEPENDENCY_SATISFIED_STATES:
                    satisfied.add((task_id, depends_on_id))
                break
    return satisfied


def unsatisfied_dependencies(session: Session, task_id: str) -> list[Task]:
    edges = [
        (task_id, depends_on_id)
        for depends_on_id in session.scalars(
            select(TaskDependency.depends_on_id).where(TaskDependency.task_id == task_id)
        )
    ]
    satisfied = satisfied_dependencies(session, edges)
    blockers = [d for _, d in edges if (task_id, d) not in satisfied]
    if not blockers:
        return []
    return list(session.scalars(select(Task).where(Task.id.in_(blockers)).order_by(Task.number)))


def depends_on_reachable(session: Session, start: str, target: str) -> list[str] | None:
    """Return a dependency path ``start -> ... -> target`` if one exists (BFS)."""
    parents: dict[str, str | None] = {start: None}
    queue: deque[str] = deque([start])
    while queue:
        current = queue.popleft()
        if current == target:
            path = [current]
            previous = parents[current]
            while previous is not None:
                path.append(previous)
                previous = parents[previous]
            return list(reversed(path))
        for nxt in session.scalars(
            select(TaskDependency.depends_on_id).where(TaskDependency.task_id == current)
        ):
            if nxt not in parents:
                parents[nxt] = current
                queue.append(nxt)
    return None


def descendants(session: Session, task_id: str) -> list[Task]:
    found: list[Task] = []
    frontier = [task_id]
    seen = {task_id}
    while frontier:
        children = list(session.scalars(select(Task).where(Task.parent_id.in_(frontier))))
        frontier = []
        for child in children:
            if child.id not in seen:
                seen.add(child.id)
                found.append(child)
                frontier.append(child.id)
    return found


def is_ancestor(session: Session, candidate_ancestor: str, task_id: str) -> bool:
    current: str | None = task_id
    seen: set[str] = set()
    while current is not None and current not in seen:
        if current == candidate_ancestor:
            return True
        seen.add(current)
        current = session.scalar(select(Task.parent_id).where(Task.id == current))
    return False


class TaskService(Service):
    # ------------------------------------------------------------------ internal helpers

    def _workspace_id(self, session: Session, workspace: str | None) -> str:
        if workspace is None:
            return self.app.default_workspace_id()
        return resolve.workspace(session, workspace).id

    def _enqueue_enrichment(self, session: Session, ctx: RequestContext, task: Task, causation: str) -> None:
        config = self.app.config.llm
        if config.auto_classify:
            jobqueue.enqueue(
                session,
                "classify_task",
                {"task_id": task.id, "reason": "material_change"},
                workspace_id=task.workspace_id,
                task_id=task.id,
                idempotency_key=f"classify:{task.id}:{task.description_hash}",
                resource_keys=["gpu"],
                max_attempts=4,
                backoff_base_seconds=15,
                correlation_id=ctx.correlation_id,
                causation_id=causation,
                created_by=ctx.actor,
            )
        if config.auto_embed:
            jobqueue.enqueue(
                session,
                "embed_task",
                {"task_id": task.id},
                workspace_id=task.workspace_id,
                task_id=task.id,
                idempotency_key=f"embed:{task.id}:{task.description_hash}",
                resource_keys=["gpu"],
                max_attempts=4,
                backoff_base_seconds=15,
                correlation_id=ctx.correlation_id,
                causation_id=causation,
                created_by=ctx.actor,
            )

    def _set_tags(self, task: Task, tags: Iterable[str]) -> None:
        wanted = list(dict.fromkeys(tags))
        task.tags = [TaskTag(task_id=task.id, tag=t) for t in wanted]

    def _set_projects(self, session: Session, task: Task, refs: Iterable[str]) -> builtins.list[str]:
        ids = [resolve.project(session, task.workspace_id, ref).id for ref in refs]
        session.execute(delete(TaskProject).where(TaskProject.task_id == task.id))
        for pid in dict.fromkeys(ids):
            session.add(TaskProject(task_id=task.id, project_id=pid))
        return sorted(set(ids))

    def _transition(
        self,
        session: Session,
        ctx: RequestContext,
        task: Task,
        target: TaskStatus,
        *,
        reason: str | None = None,
        privileged: bool = False,
        tracker: bool = False,
        extra: dict[str, Any] | None = None,
    ) -> str | None:
        """Validate and apply a status change; returns the status-change event id."""
        current = TaskStatus(task.status)
        if target in REQUIRES_DEPENDENCIES:
            blockers = unsatisfied_dependencies(session, task.id)
            if blockers:
                if target != TaskStatus.READY:
                    raise InvalidTransitionError(
                        f"task {task.key} has unsatisfied dependencies",
                        details={"blocked_by": [b.key for b in blockers]},
                    )
                target = TaskStatus.BLOCKED
                task.metadata_ = {**(task.metadata_ or {}), "blocked_reason": BLOCKED_BY_DEPENDENCIES}
                reason = reason or "dependencies unsatisfied: " + ", ".join(b.key for b in blockers)
        if current == target:
            return None
        check_task_transition(current, target, privileged=privileged, tracker=tracker)
        now = utcnow()
        task.status = target
        if target != TaskStatus.BLOCKED and (task.metadata_ or {}).get("blocked_reason"):
            task.metadata_ = {k: v for k, v in (task.metadata_ or {}).items() if k != "blocked_reason"}
        if target in (TaskStatus.CLAIMED, TaskStatus.RUNNING) and task.started_at is None:
            task.started_at = now
        if target in (TaskStatus.AWAITING_VERIFICATION, TaskStatus.AWAITING_REVIEW):
            task.completed_at = now
        if target == TaskStatus.ACCEPTED:
            task.accepted_at = now
            if task.completed_at is None:
                task.completed_at = now
        if target not in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
            task.claimed_by = None
            task.claim_expires_at = None
        reopened = current in (TaskStatus.ACCEPTED, TaskStatus.CANCELLED)
        if reopened:
            task.accepted_at = None
        task.version += 1
        task.updated_at = now
        event = record_event(
            session,
            ctx,
            event_type="task.status_changed",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            data={"from": current, "to": target, "reason": reason, "version": task.version, **(extra or {})},
        )
        if reopened:
            record_event(
                session,
                ctx.caused_by(event.id),
                event_type="task.reopened",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"from": current, "to": target, "reason": reason},
            )
        if target == TaskStatus.READY:
            record_event(
                session,
                ctx.caused_by(event.id),
                event_type="task.became_ready",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"key": task.key},
            )
        if TaskStatus.ACCEPTED in (target, current):
            self._refresh_dependents(session, ctx.caused_by(event.id), task.id)
        return event.id

    def _refresh_dependents(self, session: Session, ctx: RequestContext, task_id: str) -> None:
        dependents = session.scalars(
            select(Task)
            .join(TaskDependency, TaskDependency.task_id == Task.id)
            .where(TaskDependency.depends_on_id == task_id)
        )
        for dependent in list(dependents):
            self._refresh_blocked_state(session, ctx, dependent)

    def _refresh_blocked_state(self, session: Session, ctx: RequestContext, task: Task) -> None:
        blockers = unsatisfied_dependencies(session, task.id)
        status = TaskStatus(task.status)
        if status == TaskStatus.BLOCKED and not blockers:
            if (task.metadata_ or {}).get("blocked_reason") == BLOCKED_BY_DEPENDENCIES:
                self._transition(session, ctx, task, TaskStatus.READY, reason="all dependencies accepted")
        elif status == TaskStatus.READY and blockers:
            self._transition(session, ctx, task, TaskStatus.READY, reason="dependency reopened")

    def _guard_tracker_owned(
        self, session: Session, ctx: RequestContext, task: Task, fields: Collection[str]
    ) -> None:
        """Reject a change to tracker-owned fields of a tracked task unless the actor is the sync."""
        if ctx.actor == SYNC_ACTOR:
            return
        owned = sorted(set(fields) & TRACKER_OWNED_FIELDS)
        if not owned:
            return
        item = is_tracked(session, task.id)
        if item is None:
            return
        raise PermissionDenied(
            f"{task.key} mirrors {item.external_key} in {item.connection}; the tracker owns "
            f"{', '.join(owned)} for it, so change them there",
            details={"fields": owned, "connection": item.connection, "external_key": item.external_key},
        )

    def _touch(self, task: Task, expected_version: int | None) -> None:
        if expected_version is not None and expected_version != task.version:
            raise ConcurrencyError(
                f"task {task.key} was modified concurrently (expected version {expected_version}, "
                f"found {task.version}); re-read and retry",
                details={"expected": expected_version, "actual": task.version},
            )

    # ------------------------------------------------------------------ create / read

    def create(self, ctx: RequestContext, workspace: str | None, data: TaskCreate) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace)
            out, _ = self._create_in_session(session, ctx, ws_id, data)
            return out

    def _create_in_session(
        self, session: Session, ctx: RequestContext, ws_id: str, data: TaskCreate
    ) -> tuple[TaskOut, str]:
        ws = session.get(Workspace, ws_id)
        if ws is None:
            raise NotFoundError(f"workspace {ws_id} not found")
        if data.key is not None:
            _check_tracker_key(session, ctx, ws, data.key)
        number = ws.next_task_number
        ws.next_task_number = number + 1
        parent_id = resolve.task(session, ws.id, data.parent).id if data.parent else None
        task = Task(
            workspace_id=ws.id,
            number=number,
            key=data.key or f"{ws.key_prefix}-{number}",
            title=data.title,
            description=data.description,
            description_hash=content_hash(data.title, data.description),
            acceptance_criteria=data.acceptance_criteria,
            status=TaskStatus.BACKLOG,
            priority=data.priority,
            parent_id=parent_id,
            assignee=data.assignee,
            assigned_agent=data.assigned_agent,
            auto_execute=data.auto_execute,
            due_at=data.due_at,
            scheduled_at=data.scheduled_at,
            estimated_effort=data.estimated_effort.model_dump() if data.estimated_effort else None,
            metadata_=data.metadata,
            created_by=ctx.actor,
        )
        session.add(task)
        session.flush()
        self._set_tags(task, data.tags)
        _set_files(session, task.id, [f.model_dump() for f in data.files])
        project_ids = self._set_projects(session, task, data.projects)
        repo_ids: builtins.list[str] = []
        for ref in data.repositories:
            repo = resolve.repository(session, ws.id, ref)
            if repo.id not in repo_ids:
                session.add(TaskRepository(task_id=task.id, repository_id=repo.id, access=repo.access_mode))
                repo_ids.append(repo.id)
        dep_ids: builtins.list[str] = []
        for ref in data.depends_on:
            dep = resolve.task(session, ws.id, ref)
            if dep.id not in dep_ids:
                session.add(TaskDependency(task_id=task.id, depends_on_id=dep.id, created_by=ctx.actor))
                dep_ids.append(dep.id)
        session.flush()
        created = record_event(
            session,
            ctx,
            event_type="task.created",
            entity_type="task",
            entity_id=task.id,
            workspace_id=ws.id,
            data={
                "key": task.key,
                "title": task.title,
                "description": task.description,
                "acceptance_criteria": task.acceptance_criteria,
                "status": TaskStatus.BACKLOG,
                "priority": task.priority,
                "tags": [t.tag for t in task.tags],
                "project_ids": project_ids,
                "repository_ids": repo_ids,
                "parent_id": parent_id,
                "depends_on": dep_ids,
                "assignee": task.assignee,
                "auto_execute": task.auto_execute,
            },
        )
        if data.status == TaskStatus.READY:
            self._transition(
                session, ctx.caused_by(created.id), task, TaskStatus.READY, reason="created ready"
            )
        index_task(session, task.id)
        self._enqueue_enrichment(session, ctx, task, created.id)
        session.flush()
        return task_out(session, task), created.id

    def get(self, ctx: RequestContext, workspace: str | None, ref: str) -> TaskOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            return task_out(session, resolve.task(session, ws_id, ref))

    def detail(self, ctx: RequestContext, workspace: str | None, ref: str) -> TaskDetail:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            detail: TaskDetail = task_out(session, task, TaskDetail)
            relations = session.scalars(
                select(TaskRelation).where(
                    or_(TaskRelation.source_task_id == task.id, TaskRelation.target_task_id == task.id)
                )
            )
            detail.relations = [RelationOut.model_validate(r) for r in relations]
            comments = session.scalars(
                select(TaskComment).where(TaskComment.task_id == task.id).order_by(TaskComment.created_at)
            )
            detail.comments = [CommentOut.model_validate(c) for c in comments]
            detail.rollup = self._rollup(session, task.id)
            detail.dependency_state = self._dependency_state(session, task.id)
            return detail

    def _rollup(self, session: Session, task_id: str) -> TaskRollup | None:
        children = descendants(session, task_id)
        if not children:
            return None
        by_status: dict[str, int] = defaultdict(int)
        for child in children:
            by_status[child.status] += 1
        accepted = by_status.get(TaskStatus.ACCEPTED, 0)
        countable = len(children) - by_status.get(TaskStatus.CANCELLED, 0)
        return TaskRollup(
            total_descendants=len(children),
            by_status=dict(by_status),
            accepted=accepted,
            progress=round(accepted / countable, 4) if countable else 1.0,
        )

    def rollup(self, ctx: RequestContext, workspace: str | None, ref: str) -> TaskRollup | None:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            return self._rollup(session, resolve.task(session, ws_id, ref).id)

    def _dependency_state(self, session: Session, task_id: str) -> DependencyState:
        blockers = unsatisfied_dependencies(session, task_id)
        return DependencyState(
            satisfied=not blockers, unsatisfied=[TaskBrief.model_validate(b) for b in blockers]
        )

    def _filtered(self, session: Session, ws_id: str, flt: TaskFilter) -> Select[Task]:
        stmt = select(Task).where(Task.workspace_id == ws_id)
        if flt.archived_only:
            stmt = stmt.where(Task.archived_at.is_not(None))
        elif not flt.include_archived:
            stmt = stmt.where(Task.archived_at.is_(None))
        if flt.statuses:
            stmt = stmt.where(Task.status.in_([str(s) for s in flt.statuses]))
        if flt.priorities:
            stmt = stmt.where(Task.priority.in_([str(p) for p in flt.priorities]))
        if flt.category:
            stmt = stmt.where(Task.category == flt.category)
        if flt.assignee:
            stmt = stmt.where(Task.assignee == flt.assignee)
        if flt.project:
            proj = resolve.project(session, ws_id, flt.project)
            stmt = stmt.where(
                Task.id.in_(select(TaskProject.task_id).where(TaskProject.project_id == proj.id))
            )
        if flt.file:
            stmt = stmt.where(Task.id.in_(select(TaskFile.task_id).where(TaskFile.path == flt.file)))
        if flt.repository:
            repo = resolve.repository(session, ws_id, flt.repository, include_archived=True)
            stmt = stmt.where(
                Task.id.in_(select(TaskRepository.task_id).where(TaskRepository.repository_id == repo.id))
            )
        if flt.tags:
            for tag in flt.tags:
                stmt = stmt.where(Task.id.in_(select(TaskTag.task_id).where(TaskTag.tag == tag)))
        if flt.parent:
            parent = resolve.task(session, ws_id, flt.parent)
            stmt = stmt.where(Task.parent_id == parent.id)
        if flt.root_only:
            stmt = stmt.where(Task.parent_id.is_(None))
        if flt.created_after:
            stmt = stmt.where(Task.created_at >= flt.created_after)
        if flt.created_before:
            stmt = stmt.where(Task.created_at < flt.created_before)
        if flt.text:
            match = to_fts_query(flt.text)
            if match:
                stmt = stmt.where(
                    Task.id.in_(
                        select(text("task_id"))
                        .select_from(text("task_fts"))
                        .where(text("task_fts MATCH :match"))
                    )
                ).params(match=match)
        return stmt

    def list(
        self, ctx: RequestContext, workspace: str | None, flt: TaskFilter | None = None
    ) -> Page[TaskOut]:
        require(ctx, Permission.TASK_READ)
        flt = flt or TaskFilter()
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace)
            stmt = self._filtered(session, ws_id, flt)
            total = int(session.scalar(select(func.count()).select_from(stmt.subquery())) or 0)
            if flt.sort == "priority":
                rank = case({str(p): r for p, r in PRIORITY_RANK.items()}, value=Task.priority, else_=9)
                order = [rank.desc() if flt.descending else rank.asc(), Task.number.asc()]
            elif flt.sort == "key":
                order = [Task.number.desc() if flt.descending else Task.number.asc()]
            else:
                column = getattr(Task, flt.sort)
                order = [column.desc() if flt.descending else column.asc(), Task.number.desc()]
            rows = list(session.scalars(stmt.order_by(*order).limit(flt.limit).offset(flt.offset)))
            items = [task_out(session, t) for t in rows]
            next_offset = flt.offset + flt.limit if flt.offset + flt.limit < total else None
            return Page[TaskOut](
                items=items, total=total, limit=flt.limit, offset=flt.offset, next_offset=next_offset
            )

    def get_ready(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        project: str | None = None,
        repository: str | None = None,
        agent: str | None = None,
        limit: int = 20,
    ) -> builtins.list[TaskOut]:
        """Ready, unclaimed tasks whose dependencies are all accepted, by priority then age."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace)
            flt = TaskFilter(statuses=[TaskStatus.READY], project=project, repository=repository, limit=500)
            stmt = self._filtered(session, ws_id, flt)
            if agent:
                stmt = stmt.where(or_(Task.assigned_agent.is_(None), Task.assigned_agent == agent))
            rank = case({str(p): r for p, r in PRIORITY_RANK.items()}, value=Task.priority, else_=9)
            candidates = session.scalars(stmt.order_by(rank.asc(), Task.created_at.asc()))
            ready: builtins.list[TaskOut] = []
            for task in candidates:
                if unsatisfied_dependencies(session, task.id):
                    continue
                ready.append(task_out(session, task))
                if len(ready) >= limit:
                    break
            return ready

    # ------------------------------------------------------------------ update

    def update(self, ctx: RequestContext, workspace: str | None, ref: str, data: TaskUpdate) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            self._update_in_session(session, ctx, task, data)
            return task_out(session, task)

    def _update_in_session(self, session: Session, ctx: RequestContext, task: Task, data: TaskUpdate) -> None:
        self._touch(task, data.expected_version)
        changes: dict[str, builtins.list[Any]] = {}

        def change(field: str, attr: str, new: Any) -> None:
            old = getattr(task, attr)
            if old != new:
                changes[field] = [old, new]
                setattr(task, attr, new)

        simple = {
            "title": "title",
            "description": "description",
            "acceptance_criteria": "acceptance_criteria",
            "assignee": "assignee",
            "assigned_agent": "assigned_agent",
            "auto_execute": "auto_execute",
            "due_at": "due_at",
            "scheduled_at": "scheduled_at",
        }
        provided = data.model_dump(exclude_unset=True)
        for field, attr in simple.items():
            if field in provided:
                change(field, attr, provided[field])
        if data.priority is not None:
            change("priority", "priority", str(data.priority))
        if data.estimated_effort is not None:
            change("estimated_effort", "estimated_effort", data.estimated_effort.model_dump())
        if data.metadata is not None:
            merged = {**(task.metadata_ or {}), **data.metadata}
            change("metadata", "metadata_", merged)
        current_tags = [t.tag for t in task.tags]
        new_tags = list(current_tags)
        if data.tags is not None:
            new_tags = list(data.tags)
        for tag in data.add_tags or []:
            if tag not in new_tags:
                new_tags.append(tag)
        if data.remove_tags:
            new_tags = [t for t in new_tags if t not in data.remove_tags]
        if new_tags != current_tags:
            changes["tags"] = [current_tags, new_tags]
            self._set_tags(task, new_tags)
        current_files = _file_dicts(session, task.id)
        new_files = [f.model_dump() for f in data.files] if data.files is not None else list(current_files)
        known = {(f["path"], f["anchor"]) for f in new_files}
        for item in data.add_files or []:
            if (item.path, item.anchor) not in known:
                known.add((item.path, item.anchor))
                new_files.append(item.model_dump())
        if new_files != current_files:
            changes["files"] = [current_files, new_files]
            _set_files(session, task.id, new_files)
        if data.projects is not None:
            before = sorted(
                session.scalars(select(TaskProject.project_id).where(TaskProject.task_id == task.id))
            )
            after = self._set_projects(session, task, data.projects)
            if before != after:
                changes["project_ids"] = [before, after]
        if data.clear_parent and task.parent_id is not None:
            changes["parent_id"] = [task.parent_id, None]
            task.parent_id = None
        elif data.parent is not None:
            parent = resolve.task(session, task.workspace_id, data.parent)
            if parent.id == task.id or is_ancestor(session, task.id, parent.id):
                raise DependencyCycleError(
                    f"setting {parent.key} as parent of {task.key} would create a hierarchy cycle"
                )
            if task.parent_id != parent.id:
                changes["parent_id"] = [task.parent_id, parent.id]
                task.parent_id = parent.id

        material = False
        new_hash = content_hash(task.title, task.description)
        if new_hash != task.description_hash:
            task.description_hash = new_hash
            material = True

        touched = {"parent" if field == "parent_id" else field for field in changes}
        if data.status is not None:
            target = TaskStatus(data.status)
            current = TaskStatus(task.status)
            closed = {TaskStatus.ACCEPTED, TaskStatus.CANCELLED}
            if target != current and ({target, current} & closed):
                touched.add("status")
        self._guard_tracker_owned(session, ctx, task, touched)

        event_id: str | None = None
        if changes:
            task.version += 1
            task.updated_at = utcnow()
            event = record_event(
                session,
                ctx,
                event_type="task.updated",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={
                    "key": task.key,
                    "changes": changes,
                    "material_change": material,
                    "reason": data.reason,
                    "version": task.version,
                },
            )
            event_id = event.id
        if data.status is not None:
            target = TaskStatus(data.status)
            if target == TaskStatus.CLAIMED:
                raise ValidationFailed("use task_claim to claim a task")
            status_ctx = ctx.caused_by(event_id) if event_id else ctx
            self._transition(session, status_ctx, task, target, reason=data.reason)
        if changes or data.status is not None:
            index_task(session, task.id)
        if material and event_id:
            self._enqueue_enrichment(session, ctx, task, event_id)
        session.flush()

    def bulk_update(self, ctx: RequestContext, workspace: str | None, data: BulkUpdate) -> BulkResult:
        """Apply one update to many tasks; each task commits or fails independently."""
        updated: builtins.list[str] = []
        failed: dict[str, str] = {}
        for ref in data.tasks:
            try:
                out = self.update(
                    ctx, workspace, ref, data.update.model_copy(update={"expected_version": None})
                )
                updated.append(out.key)
            except (ValidationFailed, ConflictError, NotFoundError, PermissionDenied) as exc:
                failed[ref] = exc.message
        return BulkResult(updated=updated, failed=failed)

    def archive(
        self, ctx: RequestContext, workspace: str | None, ref: str, *, archived: bool = True
    ) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            if archived != (task.archived_at is not None):
                self._guard_tracker_owned(session, ctx, task, {"archived"})
            if archived and task.archived_at is None:
                task.archived_at = utcnow()
                event_type = "task.archived"
            elif not archived and task.archived_at is not None:
                task.archived_at = None
                event_type = "task.unarchived"
            else:
                return task_out(session, task)
            task.version += 1
            record_event(
                session,
                ctx,
                event_type=event_type,
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"key": task.key},
            )
            session.flush()
            return task_out(session, task)

    # ------------------------------------------------------------------ dependencies & relations

    def add_dependency(
        self, ctx: RequestContext, workspace: str | None, ref: str, depends_on: str
    ) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            dep = resolve.task(session, task.workspace_id, depends_on)
            if dep.id == task.id:
                raise DependencyCycleError("a task cannot depend on itself")
            exists = session.scalar(
                select(TaskDependency).where(
                    TaskDependency.task_id == task.id, TaskDependency.depends_on_id == dep.id
                )
            )
            if exists is None:
                path = depends_on_reachable(session, dep.id, task.id)
                if path is not None:
                    by_id = {
                        row.id: row.key
                        for row in session.execute(select(Task.id, Task.key).where(Task.id.in_(path)))
                    }
                    keys = [by_id[p] for p in path]
                    raise DependencyCycleError(
                        f"adding {task.key} -> {dep.key} would create a cycle: "
                        + " -> ".join([task.key, *keys]),
                        details={"cycle": [task.key, *keys]},
                    )
                session.add(TaskDependency(task_id=task.id, depends_on_id=dep.id, created_by=ctx.actor))
                task.version += 1
                session.flush()
                event = record_event(
                    session,
                    ctx,
                    event_type="task.dependency_added",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"depends_on": dep.id, "depends_on_key": dep.key},
                )
                self._refresh_blocked_state(session, ctx.caused_by(event.id), task)
            session.flush()
            return task_out(session, task)

    def remove_dependency(
        self, ctx: RequestContext, workspace: str | None, ref: str, depends_on: str
    ) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            dep = resolve.task(session, task.workspace_id, depends_on)
            edge = session.scalar(
                select(TaskDependency).where(
                    TaskDependency.task_id == task.id, TaskDependency.depends_on_id == dep.id
                )
            )
            if edge is not None and edge.created_by == SYNC_ACTOR and ctx.actor != SYNC_ACTOR:
                raise PermissionDenied(
                    f"the dependency of {task.key} on {dep.key} comes from the tracker; remove it there"
                )
            result = session.execute(
                delete(TaskDependency).where(
                    TaskDependency.task_id == task.id, TaskDependency.depends_on_id == dep.id
                )
            )
            if getattr(result, "rowcount", 0):
                task.version += 1
                event = record_event(
                    session,
                    ctx,
                    event_type="task.dependency_removed",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"depends_on": dep.id, "depends_on_key": dep.key},
                )
                self._refresh_blocked_state(session, ctx.caused_by(event.id), task)
            session.flush()
            return task_out(session, task)

    def add_relation(
        self, ctx: RequestContext, workspace: str | None, ref: str, data: RelationCreate
    ) -> RelationOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            target = resolve.task(session, task.workspace_id, data.target)
            if target.id == task.id:
                raise ValidationFailed("a task cannot relate to itself")
            relation = session.scalar(
                select(TaskRelation).where(
                    TaskRelation.source_task_id == task.id,
                    TaskRelation.target_task_id == target.id,
                    TaskRelation.relation_type == data.relation_type,
                )
            )
            if relation is None:
                relation = TaskRelation(
                    source_task_id=task.id,
                    target_task_id=target.id,
                    relation_type=data.relation_type,
                    created_by=ctx.actor,
                )
                session.add(relation)
                session.flush()
                record_event(
                    session,
                    ctx,
                    event_type="task.relation_added",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"target": target.id, "target_key": target.key, "relation_type": data.relation_type},
                )
            return RelationOut.model_validate(relation)

    # ------------------------------------------------------------------ repositories

    def link_repository(
        self, ctx: RequestContext, workspace: str | None, ref: str, repository: str, access: str | None = None
    ) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            repo = resolve.repository(session, task.workspace_id, repository)
            if access not in (None, "read_only", "read_write"):
                raise ValidationFailed("access must be read_only or read_write")
            effective = access or repo.access_mode
            if effective == "read_write" and repo.access_mode == "read_only":
                raise ValidationFailed(f"repository {repo.name} is registered read-only")
            link = session.get(TaskRepository, (task.id, repo.id))
            if link is None:
                session.add(TaskRepository(task_id=task.id, repository_id=repo.id, access=effective))
                task.version += 1
                record_event(
                    session,
                    ctx,
                    event_type="task.repository_linked",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"repository_id": repo.id, "repository": repo.name, "access": effective},
                )
            elif link.access != effective:
                link.access = effective
            session.flush()
            return task_out(session, task)

    def unlink_repository(
        self, ctx: RequestContext, workspace: str | None, ref: str, repository: str
    ) -> TaskOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            repo = resolve.repository(session, task.workspace_id, repository, include_archived=True)
            result = session.execute(
                delete(TaskRepository).where(
                    TaskRepository.task_id == task.id, TaskRepository.repository_id == repo.id
                )
            )
            if getattr(result, "rowcount", 0):
                task.version += 1
                record_event(
                    session,
                    ctx,
                    event_type="task.repository_unlinked",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"repository_id": repo.id, "repository": repo.name},
                )
            session.flush()
            return task_out(session, task)

    # ------------------------------------------------------------------ comments & reports

    def add_comment(
        self, ctx: RequestContext, workspace: str | None, ref: str, data: CommentCreate
    ) -> CommentOut:
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            comment = self._comment(session, ctx, task, data)
            return CommentOut.model_validate(comment)

    def _comment(self, session: Session, ctx: RequestContext, task: Task, data: CommentCreate) -> TaskComment:
        if data.attempt_id:
            attempt = resolve.attempt(session, data.attempt_id)
            if attempt.task_id != task.id:
                raise ValidationFailed("attempt does not belong to this task")
        comment = TaskComment(
            task_id=task.id,
            attempt_id=data.attempt_id,
            author=ctx.actor,
            kind=data.kind,
            body=data.body,
            data=data.data,
        )
        session.add(comment)
        session.flush()
        record_event(
            session,
            ctx,
            event_type={
                CommentKind.COMMENT: "task.comment_added",
                CommentKind.PROGRESS: "task.progress_reported",
                CommentKind.RESULT: "task.result_reported",
            }[CommentKind(data.kind)],
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            attempt_id=data.attempt_id,
            data={"comment_id": comment.id, "kind": data.kind, "excerpt": data.body[:500], **data.data},
        )
        index_task(session, task.id)
        return comment

    def report_progress(
        self, ctx: RequestContext, workspace: str | None, ref: str, data: ProgressReport
    ) -> CommentOut:
        require(ctx, Permission.TASK_WRITE)
        payload = dict(data.data)
        if data.percent is not None:
            payload["percent"] = data.percent
        return self.add_comment(
            ctx,
            workspace,
            ref,
            CommentCreate(
                body=data.message, kind=CommentKind.PROGRESS, data=payload, attempt_id=data.attempt_id
            ),
        )

    # ------------------------------------------------------------------ claims & completion

    def claim(self, ctx: RequestContext, workspace: str | None, ref: str, data: ClaimRequest) -> TaskOut:
        require(ctx, Permission.TASK_CLAIM)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            self._claim_in_session(session, ctx, task, data)
            return task_out(session, task)

    def _claim_in_session(
        self, session: Session, ctx: RequestContext, task: Task, data: ClaimRequest
    ) -> None:
        self._touch(task, data.expected_version)
        now = utcnow()
        claimant = f"agent:{data.agent}" if data.agent else ctx.actor
        status = TaskStatus(task.status)
        if status in (TaskStatus.CLAIMED, TaskStatus.RUNNING) and task.claimed_by:
            lease_valid = task.claim_expires_at is None or task.claim_expires_at > now
            if task.claimed_by != claimant and lease_valid:
                raise ResourceBusyError(
                    f"task {task.key} is already claimed by {task.claimed_by}",
                    details={"claimed_by": task.claimed_by, "expires_at": str(task.claim_expires_at)},
                )
            if task.claimed_by == claimant:
                task.claim_expires_at = now + timedelta(minutes=data.lease_minutes)
                task.version += 1
                return
            # Expired foreign claim: release it before re-claiming.
            self._transition(session, ctx, task, TaskStatus.READY, reason="expired claim released")
        elif status not in (TaskStatus.READY, TaskStatus.FAILED):
            raise InvalidTransitionError(
                f"task {task.key} is {status}; only ready (or failed) tasks can be claimed",
                details={"status": status},
            )
        blockers = unsatisfied_dependencies(session, task.id)
        if blockers:
            raise InvalidTransitionError(
                f"task {task.key} is blocked by {', '.join(b.key for b in blockers)}",
                details={"blocked_by": [b.key for b in blockers]},
            )
        event_id = self._transition(session, ctx, task, TaskStatus.CLAIMED, reason=f"claimed by {claimant}")
        expires_at = now + timedelta(minutes=data.lease_minutes)
        task.claimed_by = claimant
        task.claim_expires_at = expires_at
        if task.assignee is None:
            task.assignee = claimant
        record_event(
            session,
            ctx.caused_by(event_id),
            event_type="task.claimed",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            data={"claimed_by": claimant, "expires_at": expires_at.isoformat()},
        )
        session.flush()

    def release(
        self, ctx: RequestContext, workspace: str | None, ref: str, reason: str | None = None
    ) -> TaskOut:
        require(ctx, Permission.TASK_CLAIM)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            status = TaskStatus(task.status)
            if status not in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                raise InvalidTransitionError(f"task {task.key} is {status}, not claimed")
            if task.claimed_by not in (None, ctx.actor) and Permission.TASK_ADMIN not in ctx.permissions:
                raise PermissionDenied(f"task {task.key} is claimed by {task.claimed_by}")
            previous = task.claimed_by
            event_id = self._transition(session, ctx, task, TaskStatus.READY, reason=reason or "released")
            record_event(
                session,
                ctx.caused_by(event_id),
                event_type="task.released",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"previous_claimant": previous, "reason": reason},
            )
            session.flush()
            return task_out(session, task)

    def complete(
        self,
        ctx: RequestContext,
        workspace: str | None,
        ref: str,
        *,
        summary: str = "",
        attempt_id: str | None = None,
    ) -> TaskOut:
        """Agent-declared completion. Moves to awaiting verification/review - never to accepted."""
        require(ctx, Permission.TASK_COMPLETE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            self._complete_in_session(session, ctx, task, summary=summary, attempt_id=attempt_id)
            return task_out(session, task)

    def _complete_in_session(
        self, session: Session, ctx: RequestContext, task: Task, *, summary: str, attempt_id: str | None
    ) -> None:
        status = TaskStatus(task.status)
        if status not in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
            raise InvalidTransitionError(
                f"task {task.key} is {status}; claim it before completing", details={"status": status}
            )
        if (
            task.claimed_by
            and task.claimed_by != ctx.actor
            and not ctx.actor.startswith("system:")
            and Permission.TASK_ADMIN not in ctx.permissions
        ):
            raise PermissionDenied(f"task {task.key} is claimed by {task.claimed_by}")
        from workgraph.verification.configs import applicable_checks

        has_checks = bool(applicable_checks(session, task))
        target = TaskStatus.AWAITING_VERIFICATION if has_checks else TaskStatus.AWAITING_REVIEW
        event_id = self._transition(
            session, ctx, task, target, reason="completion reported", extra={"attempt_id": attempt_id}
        )
        record_event(
            session,
            ctx.caused_by(event_id),
            event_type="task.completed",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            attempt_id=attempt_id,
            data={"summary": summary[:5000], "next_status": target, "verification_configured": has_checks},
        )
        if summary:
            self._comment(
                session,
                ctx.caused_by(event_id),
                task,
                CommentCreate(body=summary, kind=CommentKind.RESULT, attempt_id=attempt_id),
            )

    def report_result(
        self, ctx: RequestContext, workspace: str | None, ref: str, data: ResultReport
    ) -> TaskOut:
        """Record an attempt's self-reported outcome. Success still requires verification/acceptance."""
        require(ctx, Permission.TASK_COMPLETE)
        with self.db.session(write=True) as session:
            ws_id = self._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            attempt: AgentAttempt | None = None
            if data.attempt_id:
                attempt = resolve.attempt(session, data.attempt_id)
                if attempt.task_id != task.id:
                    raise ValidationFailed("attempt does not belong to this task")
                from workgraph.core.services.attempts import finish_attempt

                finish_attempt(
                    session,
                    ctx,
                    attempt,
                    AttemptStatus.SUCCEEDED if data.outcome == "succeeded" else AttemptStatus.FAILED,
                    exit_reason="agent_reported",
                    summary=data.summary,
                    usage=data.usage,
                    usage_source="agent_reported" if data.usage else None,
                    cost_usd=data.cost_usd,
                    cost_source="agent_reported" if data.cost_usd is not None else None,
                )
            if data.outcome == "succeeded":
                if TaskStatus(task.status) in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                    self._complete_in_session(
                        session, ctx, task, summary=data.summary, attempt_id=data.attempt_id
                    )
                else:
                    self._comment(
                        session,
                        ctx,
                        task,
                        CommentCreate(
                            body=data.summary or "succeeded",
                            kind=CommentKind.RESULT,
                            attempt_id=data.attempt_id,
                        ),
                    )
            else:
                self._comment(
                    session,
                    ctx,
                    task,
                    CommentCreate(
                        body=data.summary or "attempt failed",
                        kind=CommentKind.RESULT,
                        attempt_id=data.attempt_id,
                        data={"outcome": "failed"},
                    ),
                )
                if TaskStatus(task.status) in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                    self._transition(session, ctx, task, TaskStatus.FAILED, reason="attempt reported failure")
            session.flush()
            return task_out(session, task)

    def apply_tracker_status(
        self, session: Session, ctx: RequestContext, task: Task, item: ExternalItem
    ) -> str | None:
        """Move a tracked task to the status its tracker issue requires; returns the status event id.

        Called by the tracker sync inside its write transaction, after it has stored the issue's
        status on ``item``.
        """
        connection = self.app.config.trackers.get(item.connection)
        cancelled = connection.cancelled_resolutions if connection is not None else []
        target = tracker_target_status(
            TaskStatus(task.status), item.tracker_status_category, item.tracker_resolution, cancelled
        )
        if target is None:
            return None
        if target == TaskStatus.ACCEPTED:
            accepted_event = self.app.verification.record_external_acceptance(
                session, ctx, task, connection=item.connection
            )
            ctx = ctx.caused_by(accepted_event)
        return self._transition(
            session,
            ctx,
            task,
            target,
            reason=f"external:{item.connection}: {item.tracker_status}",
            privileged=True,
            tracker=True,
            extra={"tracker": item.connection, "tracker_status": item.tracker_status},
        )

    def release_expired_claims(self, ctx: RequestContext) -> builtins.list[str]:
        released: builtins.list[str] = []
        with self.db.session(write=True) as session:
            expired = session.scalars(
                select(Task).where(
                    Task.status == TaskStatus.CLAIMED,
                    Task.claim_expires_at.is_not(None),
                    Task.claim_expires_at < utcnow(),
                )
            )
            for task in list(expired):
                self._transition(session, ctx, task, TaskStatus.READY, reason="claim lease expired")
                released.append(task.key)
        return released

    # ------------------------------------------------------------------ graph

    def graph(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        project: str | None = None,
        repository: str | None = None,
        root: str | None = None,
        include_closed: bool = True,
    ) -> TaskGraph:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self._workspace_id(session, workspace)
            stmt = select(Task).where(Task.workspace_id == ws_id, Task.archived_at.is_(None))
            if not include_closed:
                stmt = stmt.where(Task.status.not_in([TaskStatus.ACCEPTED, TaskStatus.CANCELLED]))
            tasks = {t.id: t for t in session.scalars(stmt)}
            if root:
                root_task = resolve.task(session, ws_id, root)
                keep = {root_task.id, *(d.id for d in descendants(session, root_task.id))}
                # include direct dependency neighbours so cross-tree blockers are visible
                deps = session.execute(
                    select(TaskDependency.task_id, TaskDependency.depends_on_id).where(
                        or_(TaskDependency.task_id.in_(keep), TaskDependency.depends_on_id.in_(keep))
                    )
                ).all()
                for a, b in deps:
                    keep.update((a, b))
                tasks = {k: v for k, v in tasks.items() if k in keep}
            if project:
                proj = resolve.project(session, ws_id, project)
                in_project = set(
                    session.scalars(select(TaskProject.task_id).where(TaskProject.project_id == proj.id))
                )
                tasks = {k: v for k, v in tasks.items() if k in in_project}
            if repository:
                repo = resolve.repository(session, ws_id, repository, include_archived=True)
                in_repo = set(
                    session.scalars(
                        select(TaskRepository.task_id).where(TaskRepository.repository_id == repo.id)
                    )
                )
                tasks = {k: v for k, v in tasks.items() if k in in_repo}
            ids = list(tasks)
            projects_by_task: dict[str, builtins.list[str]] = defaultdict(list)
            for tid, pid in session.execute(
                select(TaskProject.task_id, TaskProject.project_id).where(TaskProject.task_id.in_(ids))
            ):
                projects_by_task[tid].append(pid)
            repos_by_task: dict[str, builtins.list[str]] = defaultdict(list)
            for tid, rid in session.execute(
                select(TaskRepository.task_id, TaskRepository.repository_id).where(
                    TaskRepository.task_id.in_(ids)
                )
            ):
                repos_by_task[tid].append(rid)
            tags_by_task: dict[str, builtins.list[str]] = defaultdict(list)
            for tid, tag in session.execute(
                select(TaskTag.task_id, TaskTag.tag).where(TaskTag.task_id.in_(ids))
            ):
                tags_by_task[tid].append(tag)
            dep_rows = session.execute(
                select(TaskDependency.task_id, TaskDependency.depends_on_id).where(
                    and_(TaskDependency.task_id.in_(ids), TaskDependency.depends_on_id.in_(ids))
                )
            ).all()
            satisfied = satisfied_dependencies(session, [(tid, did) for tid, did in dep_rows])
            unsatisfied: set[str] = {tid for tid, did in dep_rows if (tid, did) not in satisfied}
            nodes = [
                GraphNode(
                    id=t.id,
                    key=t.key,
                    title=t.title,
                    status=t.status,
                    priority=t.priority,
                    category=t.category,
                    parent_id=t.parent_id if t.parent_id in tasks else None,
                    project_ids=sorted(projects_by_task[t.id]),
                    repository_ids=sorted(repos_by_task[t.id]),
                    tags=sorted(tags_by_task[t.id]),
                    blocked=t.status == TaskStatus.BLOCKED or t.id in unsatisfied,
                )
                for t in sorted(tasks.values(), key=lambda t: t.number)
            ]
            edges = [GraphEdge(source=did, target=tid, kind="depends_on") for tid, did in dep_rows]
            edges += [
                GraphEdge(source=t.parent_id, target=t.id, kind="parent")
                for t in tasks.values()
                if t.parent_id and t.parent_id in tasks
            ]
            for rel in session.scalars(
                select(TaskRelation).where(
                    TaskRelation.source_task_id.in_(ids), TaskRelation.target_task_id.in_(ids)
                )
            ):
                edges.append(
                    GraphEdge(
                        source=rel.source_task_id,
                        target=rel.target_task_id,
                        kind="relation",
                        relation_type=rel.relation_type,
                    )
                )
            return TaskGraph(nodes=nodes, edges=edges)

    # ------------------------------------------------------------------ misc

    def priorities(self) -> builtins.list[str]:
        return [str(p) for p in TaskPriority]

    def ensure_same_workspace(
        self, session: Session, task: Task, project: Project | None, repo: Repository | None
    ) -> None:
        for entity in (project, repo):
            if entity is not None and entity.workspace_id != task.workspace_id:
                raise ValidationFailed("entities from different workspaces cannot be linked")
