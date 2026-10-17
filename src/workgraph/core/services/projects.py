"""Projects: logical products/initiatives spanning many repositories."""

from __future__ import annotations

import builtins

from sqlalchemy import delete, func, select
from sqlalchemy.orm import Session

from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import TaskStatus
from workgraph.core.models.schemas import (
    ProjectCreate,
    ProjectOut,
    ProjectSummary,
    ProjectUpdate,
    RepositoryBrief,
    TaskBrief,
)
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.persistence.orm import Project, ProjectRepository, Repository, Task, TaskProject
from workgraph.security.permissions import Permission, require


def project_out(session: Session, project: Project) -> ProjectOut:
    repo_ids = list(
        session.scalars(
            select(ProjectRepository.repository_id).where(ProjectRepository.project_id == project.id)
        )
    )
    out = ProjectOut.model_validate(project)
    out.repository_ids = sorted(repo_ids)
    return out


class ProjectService(Service):
    def create(self, ctx: RequestContext, workspace: str, data: ProjectCreate) -> ProjectOut:
        require(ctx, Permission.PROJECT_WRITE)
        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, workspace)
            exists = session.scalar(
                select(Project).where(Project.workspace_id == ws.id, Project.slug == data.slug)
            )
            if exists is not None:
                raise ConflictError(f"project {data.slug!r} already exists in workspace {ws.slug}")
            project = Project(
                workspace_id=ws.id,
                slug=data.slug,
                name=data.name,
                description=data.description,
                settings=data.settings,
            )
            session.add(project)
            session.flush()
            repo_ids = []
            for ref in data.repositories:
                repo = resolve.repository(session, ws.id, ref)
                session.add(ProjectRepository(project_id=project.id, repository_id=repo.id))
                repo_ids.append(repo.id)
            record_event(
                session,
                ctx,
                event_type="project.created",
                entity_type="project",
                entity_id=project.id,
                workspace_id=ws.id,
                data={"slug": project.slug, "name": project.name, "repository_ids": repo_ids},
            )
            session.flush()
            return project_out(session, project)

    def update(self, ctx: RequestContext, workspace: str, ref: str, data: ProjectUpdate) -> ProjectOut:
        require(ctx, Permission.PROJECT_WRITE)
        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, workspace)
            project = resolve.project(session, ws.id, ref)
            changes = {}
            for field, value in data.model_dump(exclude_unset=True).items():
                if value is not None and getattr(project, field) != value:
                    changes[field] = [getattr(project, field), value]
                    setattr(project, field, value)
            if changes:
                record_event(
                    session,
                    ctx,
                    event_type="project.updated",
                    entity_type="project",
                    entity_id=project.id,
                    workspace_id=ws.id,
                    data={"changes": changes},
                )
            session.flush()
            return project_out(session, project)

    def add_repository(
        self, ctx: RequestContext, workspace: str, ref: str, repository: str, role: str = "member"
    ) -> ProjectOut:
        require(ctx, Permission.PROJECT_WRITE)
        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, workspace)
            project = resolve.project(session, ws.id, ref)
            repo = resolve.repository(session, ws.id, repository)
            if session.get(ProjectRepository, (project.id, repo.id)) is None:
                session.add(ProjectRepository(project_id=project.id, repository_id=repo.id, role=role))
                record_event(
                    session,
                    ctx,
                    event_type="project.repository_added",
                    entity_type="project",
                    entity_id=project.id,
                    workspace_id=ws.id,
                    data={"repository_id": repo.id, "role": role},
                )
            session.flush()
            return project_out(session, project)

    def remove_repository(self, ctx: RequestContext, workspace: str, ref: str, repository: str) -> ProjectOut:
        require(ctx, Permission.PROJECT_WRITE)
        with self.db.session(write=True) as session:
            ws = resolve.workspace(session, workspace)
            project = resolve.project(session, ws.id, ref)
            repo = resolve.repository(session, ws.id, repository, include_archived=True)
            result = session.execute(
                delete(ProjectRepository).where(
                    ProjectRepository.project_id == project.id, ProjectRepository.repository_id == repo.id
                )
            )
            if getattr(result, "rowcount", 0):
                record_event(
                    session,
                    ctx,
                    event_type="project.repository_removed",
                    entity_type="project",
                    entity_id=project.id,
                    workspace_id=ws.id,
                    data={"repository_id": repo.id},
                )
            session.flush()
            return project_out(session, project)

    def list(
        self, ctx: RequestContext, workspace: str, *, include_archived: bool = False
    ) -> builtins.list[ProjectOut]:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws = resolve.workspace(session, workspace)
            stmt = select(Project).where(Project.workspace_id == ws.id).order_by(Project.slug)
            if not include_archived:
                stmt = stmt.where(Project.status == "active")
            return [project_out(session, p) for p in session.scalars(stmt)]

    def get(self, ctx: RequestContext, workspace: str, ref: str) -> ProjectOut:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws = resolve.workspace(session, workspace)
            return project_out(session, resolve.project(session, ws.id, ref))

    def summary(self, ctx: RequestContext, workspace: str, ref: str) -> ProjectSummary:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws = resolve.workspace(session, workspace)
            project = resolve.project(session, ws.id, ref)
            repos = session.scalars(
                select(Repository)
                .join(ProjectRepository, ProjectRepository.repository_id == Repository.id)
                .where(ProjectRepository.project_id == project.id)
                .order_by(Repository.name)
            )
            task_scope = (
                select(Task)
                .join(TaskProject, TaskProject.task_id == Task.id)
                .where(TaskProject.project_id == project.id, Task.archived_at.is_(None))
            )
            by_status = {
                str(status): int(count)
                for status, count in session.execute(
                    select(Task.status, func.count())
                    .join(TaskProject, TaskProject.task_id == Task.id)
                    .where(TaskProject.project_id == project.id, Task.archived_at.is_(None))
                    .group_by(Task.status)
                )
            }
            recent = session.scalars(task_scope.order_by(Task.updated_at.desc()).limit(10))
            accepted = by_status.get(TaskStatus.ACCEPTED, 0)
            closed = accepted + by_status.get(TaskStatus.CANCELLED, 0)
            return ProjectSummary(
                project=project_out(session, project),
                repositories=[RepositoryBrief.model_validate(r) for r in repos],
                tasks_by_status=by_status,
                open_tasks=sum(by_status.values()) - closed,
                accepted_tasks=accepted,
                recent_tasks=[TaskBrief.model_validate(t) for t in recent],
            )
