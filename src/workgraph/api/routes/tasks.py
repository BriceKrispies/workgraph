"""Task endpoints."""

from __future__ import annotations

from datetime import datetime
from typing import Any, Literal, cast

from fastapi import APIRouter, Query
from pydantic import BaseModel, Field

from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.models.enums import TaskPriority, TaskStatus
from workgraph.core.models.schemas import (
    BulkResult,
    BulkUpdate,
    ClaimRequest,
    CommentCreate,
    CommentOut,
    EventOut,
    Page,
    ProgressReport,
    RelationCreate,
    RelationOut,
    ResultReport,
    TaskCreate,
    TaskDetail,
    TaskFilter,
    TaskGraph,
    TaskOut,
    TaskUpdate,
)

router = APIRouter(prefix="/api/v1/ws/{ws}")


@router.get("/tasks", response_model=Page[TaskOut])
def list_tasks(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    status: list[TaskStatus] | None = Query(None),
    priority: list[TaskPriority] | None = Query(None),
    project: str | None = None,
    repository: str | None = None,
    tag: list[str] | None = Query(None),
    category: str | None = None,
    assignee: str | None = None,
    parent: str | None = None,
    roots: bool = False,
    q: str | None = None,
    created_after: datetime | None = None,
    created_before: datetime | None = None,
    archived: bool = False,
    sort: str = "created_at",
    descending: bool = True,
    limit: int = Query(50, ge=1, le=500),
    offset: int = Query(0, ge=0),
) -> Any:
    return app.tasks.list(
        ctx,
        ws,
        TaskFilter(
            statuses=status,
            priorities=priority,
            project=project,
            repository=repository,
            tags=tag,
            category=category,
            assignee=assignee,
            parent=parent,
            root_only=roots,
            text=q,
            created_after=created_after,
            created_before=created_before,
            archived_only=archived,
            sort=cast(Any, sort),
            descending=descending,
            limit=limit,
            offset=offset,
        ),
    )


@router.post("/tasks", response_model=TaskOut, status_code=201)
def create_task(app: AppDep, ctx: CtxDep, ws: str, body: TaskCreate) -> Any:
    return app.tasks.create(ctx, ws, body)


@router.post("/tasks/bulk", response_model=BulkResult)
def bulk_update(app: AppDep, ctx: CtxDep, ws: str, body: BulkUpdate) -> Any:
    return app.tasks.bulk_update(ctx, ws, body)


@router.get("/ready", response_model=list[TaskOut])
def ready(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    project: str | None = None,
    repository: str | None = None,
    limit: int = Query(50, le=500),
) -> Any:
    return app.tasks.get_ready(ctx, ws, project=project, repository=repository, limit=limit)


@router.get("/graph", response_model=TaskGraph)
def graph(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    project: str | None = None,
    repository: str | None = None,
    root: str | None = None,
    include_closed: bool = True,
) -> Any:
    return app.tasks.graph(
        ctx, ws, project=project, repository=repository, root=root, include_closed=include_closed
    )


@router.get("/tasks/{ref}", response_model=TaskDetail)
def get_task(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.tasks.detail(ctx, ws, ref)


@router.patch("/tasks/{ref}", response_model=TaskOut)
def update_task(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: TaskUpdate) -> Any:
    return app.tasks.update(ctx, ws, ref, body)


@router.post("/tasks/{ref}/claim", response_model=TaskOut)
def claim(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ClaimRequest | None = None) -> Any:
    return app.tasks.claim(ctx, ws, ref, body or ClaimRequest())


class ReasonBody(BaseModel):
    reason: str = ""


@router.post("/tasks/{ref}/release", response_model=TaskOut)
def release(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ReasonBody | None = None) -> Any:
    return app.tasks.release(ctx, ws, ref, body.reason if body else None)


class CompleteBody(BaseModel):
    summary: str = ""
    attempt_id: str | None = None


@router.post("/tasks/{ref}/complete", response_model=TaskOut)
def complete(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: CompleteBody | None = None) -> Any:
    body = body or CompleteBody()
    return app.tasks.complete(ctx, ws, ref, summary=body.summary, attempt_id=body.attempt_id)


class AcceptBody(BaseModel):
    manual: bool = False
    reason: str = ""


@router.post("/tasks/{ref}/accept")
def accept(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: AcceptBody) -> Any:
    if body.manual:
        return app.verification.accept_manually(ctx, ws, ref, reason=body.reason)
    return app.verification.accept(ctx, ws, ref, reason=body.reason)


class RejectBody(BaseModel):
    reason: str = Field(min_length=1)


@router.post("/tasks/{ref}/reject")
def reject(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: RejectBody) -> Any:
    return app.verification.reject(ctx, ws, ref, reason=body.reason)


@router.get("/tasks/{ref}/acceptances")
def acceptances(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.verification.acceptances(ctx, ws, ref)


@router.post("/tasks/{ref}/comments", response_model=CommentOut, status_code=201)
def comment(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: CommentCreate) -> Any:
    return app.tasks.add_comment(ctx, ws, ref, body)


@router.post("/tasks/{ref}/progress", response_model=CommentOut)
def progress(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ProgressReport) -> Any:
    return app.tasks.report_progress(ctx, ws, ref, body)


@router.post("/tasks/{ref}/result", response_model=TaskOut)
def result(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ResultReport) -> Any:
    return app.tasks.report_result(ctx, ws, ref, body)


class DependencyBody(BaseModel):
    depends_on: str


@router.post("/tasks/{ref}/dependencies", response_model=TaskOut)
def add_dependency(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: DependencyBody) -> Any:
    return app.tasks.add_dependency(ctx, ws, ref, body.depends_on)


@router.delete("/tasks/{ref}/dependencies/{depends_on}", response_model=TaskOut)
def remove_dependency(app: AppDep, ctx: CtxDep, ws: str, ref: str, depends_on: str) -> Any:
    return app.tasks.remove_dependency(ctx, ws, ref, depends_on)


@router.post("/tasks/{ref}/relations", response_model=RelationOut)
def add_relation(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: RelationCreate) -> Any:
    return app.tasks.add_relation(ctx, ws, ref, body)


class RepoBody(BaseModel):
    repository: str
    access: str | None = None


@router.post("/tasks/{ref}/repositories", response_model=TaskOut)
def link_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: RepoBody) -> Any:
    return app.tasks.link_repository(ctx, ws, ref, body.repository, body.access)


@router.delete("/tasks/{ref}/repositories/{repository}", response_model=TaskOut)
def unlink_repository(app: AppDep, ctx: CtxDep, ws: str, ref: str, repository: str) -> Any:
    return app.tasks.unlink_repository(ctx, ws, ref, repository)


@router.post("/tasks/{ref}/archive", response_model=TaskOut)
def archive(app: AppDep, ctx: CtxDep, ws: str, ref: str, undo: bool = False) -> Any:
    return app.tasks.archive(ctx, ws, ref, archived=not undo)


@router.get("/tasks/{ref}/history", response_model=list[EventOut])
def history(app: AppDep, ctx: CtxDep, ws: str, ref: str, limit: int = Query(500, le=5000)) -> Any:
    return app.history.task_history(ctx, ws, ref, limit=limit)


@router.get("/tasks/{ref}/attempts")
def task_attempts(
    app: AppDep, ctx: CtxDep, ws: str, ref: str, limit: int = Query(50, le=500), offset: int = 0
) -> Any:
    return app.attempts.list(ctx, ws, task=ref, limit=limit, offset=offset)


class LinkCommitBody(BaseModel):
    sha: str = Field(min_length=4, max_length=64)
    repository: str | None = None
    note: str = ""


@router.get("/tasks/{ref}/commits")
def task_commits(app: AppDep, ctx: CtxDep, ws: str, ref: str, min_confidence: float = 0.0) -> Any:
    return app.git.task_commits(ctx, ws, ref, min_confidence=min_confidence)


@router.post("/tasks/{ref}/commits")
def link_commit(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: LinkCommitBody) -> Any:
    return app.git.link_commit(ctx, ws, ref, body.sha, repository=body.repository, note=body.note)


@router.get("/tasks/{ref}/changes")
def task_changes(app: AppDep, ctx: CtxDep, ws: str, ref: str, min_confidence: float = 0.5) -> Any:
    return app.git.task_changes(ctx, ws, ref, min_confidence=min_confidence)


@router.get("/tasks/{ref}/diff")
def task_diff(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    ref: str,
    repository: str | None = None,
    max_bytes: int = Query(400_000, le=5_000_000),
) -> Any:
    return app.git.task_diff(ctx, ws, ref, repository=repository, max_bytes=max_bytes)


@router.get("/tasks/{ref}/change-sets")
def change_sets(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.git.change_sets(ctx, ws, ref)


@router.get("/tasks/{ref}/classification")
def classification(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.classification.get(ctx, ws, ref)


@router.post("/tasks/{ref}/classification")
def request_classification(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    ref: str,
    provider: str | None = None,
    model: str | None = None,
    promote: bool = False,
) -> Any:
    return app.classification.request(ctx, ws, ref, provider=provider, model=model, promote=promote)


class ConfirmBody(BaseModel):
    category: str
    note: str = ""
    complexity: str | None = None


@router.post("/tasks/{ref}/classification/confirm")
def confirm_classification(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: ConfirmBody) -> Any:
    return app.classification.confirm(
        ctx, ws, ref, category=body.category, note=body.note, complexity=body.complexity
    )


@router.get("/tasks/{ref}/similar")
def similar(app: AppDep, ctx: CtxDep, ws: str, ref: str, limit: int = Query(10, le=50)) -> Any:
    return app.search.similar(ctx, ws, ref, limit=limit)


@router.get("/tasks/{ref}/estimate")
def estimate(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.analytics.estimate(ctx, ws, task=ref)


@router.get("/tasks/{ref}/verification")
def verification_runs(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.verification.list_runs(ctx, ws, task=ref)


@router.get("/tasks/{ref}/checks")
def applicable_checks(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.verification.checks_for_task(ctx, ws, ref)


@router.post("/tasks/{ref}/verify")
def verify(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    ref: str,
    attempt_id: str | None = None,
    tier: Literal["fast", "full"] | None = None,
) -> Any:
    from workgraph.jobs import queue as jobqueue
    from workgraph.security.permissions import Permission, require

    require(ctx, Permission.VERIFY_RUN)
    task = app.tasks.get(ctx, ws, ref)
    with app.db.session(write=True) as session:
        job = jobqueue.enqueue(
            session,
            "verify",
            {"task_id": task.id, "attempt_id": attempt_id, "tier": tier},
            workspace_id=task.workspace_id,
            task_id=task.id,
            max_attempts=1,
            created_by=ctx.actor,
        )
        return {"job_id": job.id}


@router.post("/tasks/{ref}/decompose")
def decompose(app: AppDep, ctx: CtxDep, ws: str, ref: str, create: bool = False) -> Any:
    task = app.tasks.get(ctx, ws, ref)
    return app.scheduler.decompose(ctx, task.id, create=create)


@router.get("/tasks/{ref}/schedulability")
def schedulability(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.scheduler.explain(ctx, ws, ref)
