"""Read access to the immutable event log and projection verification."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.events.log import query_events
from workgraph.core.events.projection import project_tasks
from workgraph.core.models.schemas import EventOut
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.persistence.orm import AgentAttempt, Task, TaskDependency, TaskProject, TaskRepository
from workgraph.security.permissions import Permission, require


class HistoryService(Service):
    def task_history(
        self,
        ctx: RequestContext,
        workspace: str | None,
        ref: str,
        *,
        include_attempts: bool = True,
        event_types: list[str] | None = None,
        limit: int = 500,
    ) -> list[EventOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, ref)
            ids = [task.id]
            if include_attempts:
                ids += list(session.scalars(select(AgentAttempt.id).where(AgentAttempt.task_id == task.id)))
            events = query_events(session, entity_ids=ids, event_types=event_types, limit=limit)
            return [EventOut.model_validate(e.to_dict()) for e in events]

    def events(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        entity_type: str | None = None,
        entity_id: str | None = None,
        event_types: list[str] | None = None,
        event_type_prefix: str | None = None,
        actor: str | None = None,
        attempt_id: str | None = None,
        correlation_id: str | None = None,
        after_seq: int | None = None,
        since: datetime | None = None,
        until: datetime | None = None,
        limit: int = 200,
        descending: bool = False,
    ) -> list[EventOut]:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws_id = resolve.workspace(session, workspace).id if workspace else None
            events = query_events(
                session,
                workspace_id=ws_id,
                entity_type=entity_type,
                entity_ids=[entity_id] if entity_id else None,
                event_types=event_types,
                event_type_prefix=event_type_prefix,
                actor=actor,
                attempt_id=attempt_id,
                correlation_id=correlation_id,
                after_seq=after_seq,
                since=since,
                until=until,
                limit=min(limit, 5000),
                descending=descending,
            )
            return [EventOut.model_validate(e.to_dict()) for e in events]

    def verify_task_projection(self, ctx: RequestContext, workspace: str) -> dict[str, Any]:
        """Replay task events and compare with current-state tables."""
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws = resolve.workspace(session, workspace)
            events = query_events(session, workspace_id=ws.id, entity_type="task", limit=10_000_000)
            projected = project_tasks(events)
            mismatches: dict[str, dict[str, Any]] = {}
            tasks = list(session.scalars(select(Task).where(Task.workspace_id == ws.id)))
            for task in tasks:
                actual = {
                    "key": task.key,
                    "title": task.title,
                    "description": task.description,
                    "status": task.status,
                    "priority": task.priority,
                    "tags": sorted(t.tag for t in task.tags),
                    "project_ids": sorted(
                        session.scalars(select(TaskProject.project_id).where(TaskProject.task_id == task.id))
                    ),
                    "repository_ids": sorted(
                        session.scalars(
                            select(TaskRepository.repository_id).where(TaskRepository.task_id == task.id)
                        )
                    ),
                    "parent_id": task.parent_id,
                    "depends_on": sorted(
                        session.scalars(
                            select(TaskDependency.depends_on_id).where(TaskDependency.task_id == task.id)
                        )
                    ),
                    "archived": task.archived_at is not None,
                }
                proj = projected.get(task.id)
                if proj is None:
                    mismatches[task.key] = {"error": "no task.created event"}
                    continue
                expected = proj.comparable()
                diff = {
                    k: {"table": actual[k], "events": expected[k]} for k in actual if actual[k] != expected[k]
                }
                if diff:
                    mismatches[task.key] = diff
            return {
                "tasks": len(tasks),
                "events": len(events),
                "consistent": not mismatches,
                "mismatches": mismatches,
            }
