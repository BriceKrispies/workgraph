"""Workspace lifecycle and summaries."""

from __future__ import annotations

import builtins
from typing import Any

from sqlalchemy import func, select

from workgraph.config.settings import WorkspaceOverrides
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import AttemptStatus, JobStatus, TaskStatus
from workgraph.core.models.schemas import WorkspaceCreate, WorkspaceOut, WorkspaceSummary
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.persistence.orm import AgentAttempt, Job, Project, Repository, Task, Workspace
from workgraph.security.permissions import Permission, require

# Workspace settings stored in the database (non-secret, operational).
ALLOWED_SETTINGS = {"allowed_roots", "default_project", "description_classification"}


class WorkspaceService(Service):
    def create(
        self, ctx: RequestContext, data: WorkspaceCreate, *, make_default: bool = False
    ) -> WorkspaceOut:
        require(ctx, Permission.WORKSPACE_ADMIN)
        with self.db.session(write=True) as session:
            if session.scalar(select(Workspace).where(Workspace.slug == data.slug)) is not None:
                raise ConflictError(f"workspace {data.slug!r} already exists")
            ws = Workspace(
                slug=data.slug, name=data.name, description=data.description, key_prefix=data.key_prefix
            )
            session.add(ws)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="workspace.created",
                entity_type="workspace",
                entity_id=ws.id,
                workspace_id=ws.id,
                data={"slug": ws.slug, "name": ws.name, "key_prefix": ws.key_prefix},
            )
            out = WorkspaceOut.model_validate(ws)
        store = self.app.config_store
        if not store.workspace_file(data.slug).exists():
            store.save_workspace_overrides(data.slug, WorkspaceOverrides())
        if make_default or self.app.config.default_workspace is None:
            self.set_default(ctx, data.slug)
        return out

    def set_default(self, ctx: RequestContext, ref: str) -> WorkspaceOut:
        require(ctx, Permission.WORKSPACE_ADMIN)
        ws = self.get(ctx, ref)
        config = self.app.config
        config.default_workspace = ws.slug
        self.app.config_store.save_global(config)
        return ws

    def list(self, ctx: RequestContext, *, include_archived: bool = False) -> builtins.list[WorkspaceOut]:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            stmt = select(Workspace).order_by(Workspace.slug)
            if not include_archived:
                stmt = stmt.where(Workspace.archived_at.is_(None))
            return [WorkspaceOut.model_validate(w) for w in session.scalars(stmt)]

    def get(self, ctx: RequestContext, ref: str) -> WorkspaceOut:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            return WorkspaceOut.model_validate(resolve.workspace(session, ref))

    def update_settings(self, ctx: RequestContext, ref: str, settings: dict[str, Any]) -> WorkspaceOut:
        require(ctx, Permission.WORKSPACE_ADMIN)
        unknown = set(settings) - ALLOWED_SETTINGS
        if unknown:
            raise ValidationFailed(f"unknown workspace settings: {sorted(unknown)}")
        if "allowed_roots" in settings:
            roots = settings["allowed_roots"]
            if not isinstance(roots, list) or not all(isinstance(r, str) for r in roots):
                raise ValidationFailed("allowed_roots must be a list of directory paths")
        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, ref)
            before = dict(ws.settings or {})
            ws.settings = {**before, **settings}
            record_event(
                session,
                ctx,
                event_type="workspace.settings_changed",
                entity_type="workspace",
                entity_id=ws.id,
                workspace_id=ws.id,
                data={"before": before, "after": ws.settings},
            )
            session.flush()
            return WorkspaceOut.model_validate(ws)

    def archive(self, ctx: RequestContext, ref: str) -> WorkspaceOut:
        require(ctx, Permission.WORKSPACE_ADMIN)
        from workgraph.core.clock import utcnow

        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, ref)
            ws.archived_at = utcnow()
            record_event(
                session,
                ctx,
                event_type="workspace.archived",
                entity_type="workspace",
                entity_id=ws.id,
                workspace_id=ws.id,
            )
            session.flush()
            return WorkspaceOut.model_validate(ws)

    def summary(self, ctx: RequestContext, ref: str) -> WorkspaceSummary:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws = resolve.workspace(session, ref)
            projects = session.scalar(select(func.count()).where(Project.workspace_id == ws.id)) or 0
            repos = (
                session.scalar(
                    select(func.count()).where(
                        Repository.workspace_id == ws.id, Repository.archived_at.is_(None)
                    )
                )
                or 0
            )
            by_status = {
                str(status): int(count)
                for status, count in session.execute(
                    select(Task.status, func.count())
                    .where(Task.workspace_id == ws.id, Task.archived_at.is_(None))
                    .group_by(Task.status)
                )
            }
            running = (
                session.scalar(
                    select(func.count()).where(
                        AgentAttempt.workspace_id == ws.id,
                        AgentAttempt.status.in_(
                            [AttemptStatus.RUNNING, AttemptStatus.PREPARING, AttemptStatus.VERIFYING]
                        ),
                    )
                )
                or 0
            )
            queued = (
                session.scalar(
                    select(func.count()).where(Job.workspace_id == ws.id, Job.status == JobStatus.QUEUED)
                )
                or 0
            )
            dead = (
                session.scalar(
                    select(func.count()).where(Job.workspace_id == ws.id, Job.status == JobStatus.DEAD)
                )
                or 0
            )
            return WorkspaceSummary(
                workspace=WorkspaceOut.model_validate(ws),
                projects=int(projects),
                repositories=int(repos),
                tasks_by_status=by_status,
                ready_tasks=by_status.get(TaskStatus.READY, 0),
                running_attempts=int(running),
                queued_jobs=int(queued),
                dead_jobs=int(dead),
            )
