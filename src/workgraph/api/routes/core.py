"""Workspaces, projects and repositories."""

from __future__ import annotations

from typing import Any

from fastapi import APIRouter, Query
from pydantic import BaseModel

from workgraph import __version__
from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.models.schemas import (
    ProjectCreate,
    ProjectOut,
    ProjectSummary,
    ProjectUpdate,
    RepositoryOut,
    RepositoryRegister,
    RepositoryStatus,
    RepositoryUpdate,
    WorkspaceCreate,
    WorkspaceOut,
    WorkspaceSummary,
)

router = APIRouter(prefix="/api/v1")


@router.get("/health")
def health() -> dict[str, str]:
    return {"status": "ok", "version": __version__}


@router.get("/meta")
def meta(app: AppDep) -> dict[str, Any]:
    config = app.config
    return {
        "version": __version__,
        "default_workspace": config.default_workspace,
        "execution_enabled": config.execution.enabled,
        "auto_schedule": config.execution.auto_schedule,
        "agents": list(config.agents),
        "llm": {"classification": config.llm.classification.model_dump(),
                "embeddings": config.llm.embeddings.model_dump()},
    }


@router.get("/workspaces", response_model=list[WorkspaceOut])
def list_workspaces(app: AppDep, ctx: CtxDep) -> Any:
    return app.workspaces.list(ctx)


@router.post("/workspaces", response_model=WorkspaceOut, status_code=201)
def create_workspace(app: AppDep, ctx: CtxDep, body: WorkspaceCreate) -> Any:
    return app.workspaces.create(ctx, body)


@router.get("/workspaces/{ws}", response_model=WorkspaceSummary)
def workspace_summary(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.workspaces.summary(ctx, ws)


@router.get("/ws/{ws}/projects", response_model=list[ProjectOut])
def list_projects(app: AppDep, ctx: CtxDep, ws: str, include_archived: bool = False) -> Any:
    return app.projects.list(ctx, ws, include_archived=include_archived)


@router.post("/ws/{ws}/projects", response_model=ProjectOut, status_code=201)
def create_project(app: AppDep, ctx: CtxDep, ws: str, body: ProjectCreate) -> Any:
    return app.projects.create(ctx, ws, body)


@router.get("/ws/{ws}/projects/{ref}", response_model=ProjectSummary)
def project_summary(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.projects.summary(ctx, ws, ref)


@router.patch("/ws/{ws}/projects/{ref}", response_model=ProjectOut)
def update_project(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ProjectUpdate) -> Any:
    return app.projects.update(ctx, ws, ref, body)


class RepoLink(BaseModel):
    repository: str
    role: str = "member"


@router.post("/ws/{ws}/projects/{ref}/repositories", response_model=ProjectOut)
def project_add_repo(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: RepoLink) -> Any:
    return app.projects.add_repository(ctx, ws, ref, body.repository, body.role)


@router.delete("/ws/{ws}/projects/{ref}/repositories/{repository}", response_model=ProjectOut)
def project_remove_repo(app: AppDep, ctx: CtxDep, ws: str, ref: str, repository: str) -> Any:
    return app.projects.remove_repository(ctx, ws, ref, repository)


@router.get("/ws/{ws}/repositories", response_model=list[RepositoryOut])
def list_repositories(app: AppDep, ctx: CtxDep, ws: str, include_archived: bool = False,
                      project: str | None = None) -> Any:
    return app.repositories.list(ctx, ws, include_archived=include_archived, project=project)


@router.post("/ws/{ws}/repositories", response_model=RepositoryOut, status_code=201)
def register_repository(app: AppDep, ctx: CtxDep, ws: str, body: RepositoryRegister) -> Any:
    return app.repositories.register(ctx, ws, body)


@router.get("/ws/{ws}/repositories/{ref}", response_model=RepositoryOut)
def get_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.repositories.get(ctx, ws, ref)


@router.patch("/ws/{ws}/repositories/{ref}", response_model=RepositoryOut)
def update_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: RepositoryUpdate) -> Any:
    return app.repositories.update(ctx, ws, ref, body)


@router.delete("/ws/{ws}/repositories/{ref}", response_model=RepositoryOut)
def remove_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.repositories.remove(ctx, ws, ref)


@router.post("/ws/{ws}/repositories/{ref}/refresh", response_model=RepositoryOut)
def refresh_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.repositories.refresh(ctx, ws, ref)


@router.get("/ws/{ws}/repositories/{ref}/status", response_model=RepositoryStatus)
def repository_status(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.repositories.status(ctx, ws, ref)


@router.post("/ws/{ws}/repositories/{ref}/sync")
def sync_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str, mode: str = Query("incremental")) -> Any:
    return app.git.sync_ref(ctx, ws, ref, mode=mode)


@router.get("/ws/{ws}/repositories/{ref}/commits")
def repository_commits(app: AppDep, ctx: CtxDep, ws: str, ref: str, limit: int = Query(50, le=500),
                       offset: int = 0, linked_only: bool = False, include_unreachable: bool = False) -> Any:
    return app.git.repository_commits(ctx, ws, ref, limit=limit, offset=offset, linked_only=linked_only,
                                      include_unreachable=include_unreachable)


@router.get("/ws/{ws}/repositories/{ref}/commits/{sha}")
def commit_detail(app: AppDep, ctx: CtxDep, ws: str, ref: str, sha: str) -> Any:
    return app.git.commit_detail(ctx, ws, ref, sha)


@router.get("/ws/{ws}/worktrees")
def list_worktrees(app: AppDep, ctx: CtxDep, ws: str, status: str | None = None) -> Any:
    return app.worktrees.list(ctx, ws, status=status)
