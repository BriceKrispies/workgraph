"""Resolution of human-friendly references (slugs, keys, names) to entities."""

from __future__ import annotations

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import is_uuid
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.persistence.orm import AgentAttempt, Project, Repository, Task, Workspace


def workspace(session: Session, ref: str) -> Workspace:
    ws = session.get(Workspace, ref) if is_uuid(ref) else None
    if ws is None:
        ws = session.scalar(select(Workspace).where(Workspace.slug == ref))
    if ws is None:
        raise NotFoundError(f"workspace {ref!r} not found", details={"ref": ref})
    return ws


def project(session: Session, workspace_id: str, ref: str) -> Project:
    proj = session.get(Project, ref) if is_uuid(ref) else None
    if proj is None or proj.workspace_id != workspace_id:
        proj = session.scalar(
            select(Project).where(Project.workspace_id == workspace_id, Project.slug == ref)
        )
    if proj is None:
        raise NotFoundError(f"project {ref!r} not found in workspace", details={"ref": ref})
    return proj


def repository(
    session: Session, workspace_id: str, ref: str, *, include_archived: bool = False
) -> Repository:
    repo = session.get(Repository, ref) if is_uuid(ref) else None
    if repo is None or repo.workspace_id != workspace_id:
        repo = session.scalar(
            select(Repository).where(Repository.workspace_id == workspace_id, Repository.name == ref)
        )
    if repo is None or (repo.archived_at is not None and not include_archived):
        raise NotFoundError(f"repository {ref!r} not found in workspace", details={"ref": ref})
    return repo


def task(session: Session, workspace_id: str | None, ref: str) -> Task:
    """Resolve a task by UUID or key (e.g. ``WG-104``, case-insensitive)."""
    if not ref:
        raise ValidationFailed("task reference must not be empty")
    found = session.get(Task, ref) if is_uuid(ref) else None
    if found is None:
        stmt = select(Task).where(func.upper(Task.key) == ref.strip().upper())
        if workspace_id is not None:
            stmt = stmt.where(Task.workspace_id == workspace_id)
        matches = list(session.scalars(stmt.limit(2)))
        if len(matches) > 1:
            raise ValidationFailed(f"task key {ref!r} is ambiguous across workspaces; pass a workspace")
        found = matches[0] if matches else None
    if found is None or (workspace_id is not None and found.workspace_id != workspace_id):
        raise NotFoundError(f"task {ref!r} not found", details={"ref": ref})
    return found


def attempt(session: Session, ref: str) -> AgentAttempt:
    found = session.get(AgentAttempt, ref)
    if found is None:
        raise NotFoundError(f"attempt {ref!r} not found", details={"ref": ref})
    return found
