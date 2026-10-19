"""Attempts, runs, jobs, workers, verification, automation, analytics, search, settings and events."""

from __future__ import annotations

import asyncio
import json
import tempfile
from datetime import datetime
from pathlib import Path
from typing import Any, Literal

from fastapi import APIRouter, Query, Request
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field
from sse_starlette.sse import EventSourceResponse

from workgraph.analytics.service import AnalyticsQuery
from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.errors import ValidationFailed
from workgraph.search.service import SearchFilters
from workgraph.security.permissions import Permission, require

router = APIRouter(prefix="/api/v1")


# ---------------------------------------------------------------------------- attempts & runs


class RunStart(BaseModel):
    task: str
    agent: str | None = None
    model: str | None = None
    instructions: str | None = Field(default=None, max_length=10_000)


@router.get("/ws/{ws}/attempts")
def list_attempts(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    task: str | None = None,
    status: list[str] | None = Query(None),
    provider: str | None = None,
    limit: int = Query(50, le=500),
    offset: int = 0,
) -> Any:
    return app.attempts.list(
        ctx, ws, task=task, statuses=status, provider=provider, limit=limit, offset=offset
    )


@router.post("/ws/{ws}/runs", status_code=201)
def start_run(app: AppDep, ctx: CtxDep, ws: str, body: RunStart) -> Any:
    return app.runs.start(
        ctx, ws, body.task, agent=body.agent, model=body.model, instructions=body.instructions
    )


@router.get("/attempts/compare")
def compare_attempts(app: AppDep, ctx: CtxDep, a: str, b: str) -> Any:
    return app.attempts.compare(ctx, a, b)


@router.get("/attempts/{attempt_id}")
def get_run(app: AppDep, ctx: CtxDep, attempt_id: str, events: int = Query(100, le=1000)) -> Any:
    return app.runs.get(ctx, attempt_id, events=events)


@router.get("/attempts/{attempt_id}/manifests")
def manifests(app: AppDep, ctx: CtxDep, attempt_id: str) -> Any:
    return app.attempts.manifests(ctx, attempt_id)


@router.get("/attempts/{attempt_id}/log")
def attempt_log(app: AppDep, ctx: CtxDep, attempt_id: str, tail: int = Query(400, le=5000)) -> Any:
    require(ctx, Permission.TASK_READ)
    attempt = app.attempts.get(ctx, attempt_id)
    path = app.paths.artifacts_dir / attempt.workspace_id / "runs" / attempt_id / "agent.log"
    if not path.exists():
        return {"lines": [], "available": False}
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return {"lines": lines[-tail:], "available": True, "total_lines": len(lines)}


class CancelBody(BaseModel):
    reason: str = "cancelled from the web UI"


@router.post("/attempts/{attempt_id}/cancel")
def cancel_run(app: AppDep, ctx: CtxDep, attempt_id: str, body: CancelBody | None = None) -> Any:
    return app.runs.cancel(ctx, attempt_id, (body or CancelBody()).reason)


@router.post("/attempts/{attempt_id}/retry")
def retry_run(app: AppDep, ctx: CtxDep, attempt_id: str) -> Any:
    return app.runs.retry(ctx, attempt_id)


@router.post("/attempts/{attempt_id}/rerun")
def rerun(app: AppDep, ctx: CtxDep, attempt_id: str) -> Any:
    return app.runs.rerun(ctx, attempt_id)


# ---------------------------------------------------------------------------- jobs & workers


def _job(job: Any) -> dict[str, Any]:
    return {
        c: getattr(job, c)
        for c in (
            "id",
            "workspace_id",
            "kind",
            "status",
            "priority",
            "payload",
            "result",
            "run_at",
            "attempts",
            "max_attempts",
            "lease_owner",
            "lease_expires_at",
            "heartbeat_at",
            "started_at",
            "finished_at",
            "cancel_requested",
            "resource_keys",
            "last_error",
            "error_kind",
            "task_id",
            "attempt_id",
            "created_by",
            "created_at",
        )
    }


@router.get("/jobs")
def list_jobs(
    app: AppDep,
    ctx: CtxDep,
    workspace: str | None = None,
    status: list[str] | None = Query(None),
    kind: str | None = None,
    task: str | None = None,
    limit: int = Query(100, le=1000),
    offset: int = 0,
) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    ws_id = app.workspaces.get(ctx, workspace).id if workspace else None
    jobs, total = app.queue.list(
        statuses=status, kind=kind, workspace_id=ws_id, task_id=task, limit=limit, offset=offset
    )
    return {"items": [_job(j) for j in jobs], "total": total, "stats": app.queue.stats(ws_id)}


@router.get("/jobs/{job_id}")
def get_job(app: AppDep, ctx: CtxDep, job_id: str) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    runs = [
        {
            "attempt": r.attempt_number,
            "worker": r.worker_id,
            "status": r.status,
            "error": r.error,
            "started_at": r.started_at,
            "finished_at": r.finished_at,
        }
        for r in app.queue.runs(job_id)
    ]
    return {**_job(app.queue.get(job_id)), "runs": runs}


@router.post("/jobs/{job_id}/cancel")
def cancel_job(app: AppDep, ctx: CtxDep, job_id: str) -> Any:
    require(ctx, Permission.JOB_MANAGE)
    return {"status": app.queue.cancel(job_id)}


@router.post("/jobs/{job_id}/retry")
def retry_job(app: AppDep, ctx: CtxDep, job_id: str) -> Any:
    require(ctx, Permission.JOB_MANAGE)
    app.queue.retry_dead(job_id)
    return {"status": "queued"}


@router.get("/workers")
def workers(app: AppDep, ctx: CtxDep) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    return [
        {
            c: getattr(w, c)
            for c in (
                "id",
                "hostname",
                "pid",
                "status",
                "concurrency",
                "current_jobs",
                "jobs_completed",
                "jobs_failed",
                "started_at",
                "heartbeat_at",
                "stopped_at",
            )
        }
        for w in app.queue.workers()
    ]


# ---------------------------------------------------------------------------- verification


class VerificationConfigBody(BaseModel):
    scope_type: Literal["workspace", "project", "repository", "task"]
    scope: str | None = None
    name: str
    checks: list[dict[str, Any]]


@router.get("/ws/{ws}/verification/configs")
def verification_configs(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.verification.list_configs(ctx, ws)


@router.post("/ws/{ws}/verification/configs", status_code=201)
def create_verification_config(app: AppDep, ctx: CtxDep, ws: str, body: VerificationConfigBody) -> Any:
    return app.verification.create_config(
        ctx, ws, scope_type=body.scope_type, scope=body.scope, name=body.name, checks=body.checks
    )


@router.post("/verification/configs/{config_id}/{action}")
def toggle_verification_config(
    app: AppDep, ctx: CtxDep, config_id: str, action: Literal["enable", "disable"]
) -> Any:
    return app.verification.set_config_enabled(ctx, config_id, action == "enable")


@router.get("/ws/{ws}/verification/runs")
def verification_runs(
    app: AppDep, ctx: CtxDep, ws: str, status: str | None = None, limit: int = Query(50, le=500)
) -> Any:
    return app.verification.list_runs(ctx, ws, status=status, limit=limit)


@router.get("/verification/runs/{run_id}")
def verification_run(app: AppDep, ctx: CtxDep, run_id: str) -> Any:
    return app.verification.get_run(ctx, run_id)


@router.get("/ws/{ws}/verification/failures")
def verification_failures(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    check: str | None = None,
    repository: str | None = None,
    since: datetime | None = None,
) -> Any:
    return app.verification.failures(ctx, ws, check_name=check, repository=repository, since=since)


# ---------------------------------------------------------------------------- automation


@router.get("/automation/catalog")
def automation_catalog(app: AppDep) -> Any:
    return app.automation.catalog()


@router.get("/ws/{ws}/automation/rules")
def list_rules(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.automation.list_rules(ctx, ws)


@router.post("/ws/{ws}/automation/rules", status_code=201)
def create_rule(app: AppDep, ctx: CtxDep, ws: str, body: dict[str, Any]) -> Any:
    return app.automation.create_rule(ctx, ws, body)


@router.get("/ws/{ws}/automation/rules/{ref}")
def get_rule(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.automation.get_rule(ctx, ws, ref)


@router.put("/ws/{ws}/automation/rules/{ref}")
def update_rule(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: dict[str, Any]) -> Any:
    return app.automation.update_rule(ctx, ws, ref, body)


@router.post("/ws/{ws}/automation/rules/{ref}/{action}")
def rule_action(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    ref: str,
    action: Literal["enable", "disable", "trigger"],
    body: dict[str, Any] | None = None,
) -> Any:
    if action == "trigger":
        return app.automation.trigger(ctx, ws, ref, inputs=(body or {}).get("inputs"))
    return app.automation.set_enabled(ctx, ws, ref, action == "enable")


class PreviewBody(BaseModel):
    rule: str | None = None
    spec: dict[str, Any] | None = None
    event_id: str | None = None
    inputs: dict[str, Any] | None = None


@router.post("/ws/{ws}/automation/preview")
def preview_rule(app: AppDep, ctx: CtxDep, ws: str, body: PreviewBody) -> Any:
    return app.automation.preview(
        ctx, ws, rule=body.rule, spec=body.spec, event_id=body.event_id, inputs=body.inputs
    )


@router.get("/ws/{ws}/automation/runs")
def automation_runs(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    rule: str | None = None,
    status: str | None = None,
    limit: int = Query(100, le=1000),
) -> Any:
    return app.automation.runs(ctx, ws, rule=rule, status=status, limit=limit)


@router.get("/ws/{ws}/notifications")
def notifications(app: AppDep, ctx: CtxDep, ws: str, unread: bool = False) -> Any:
    return app.automation.notifications(ctx, ws, unread_only=unread)


@router.post("/notifications/{notification_id}/read")
def read_notification(app: AppDep, ctx: CtxDep, notification_id: str) -> Any:
    app.automation.mark_read(ctx, notification_id)
    return {"ok": True}


# ---------------------------------------------------------------------------- analytics, search, reports


@router.get("/analytics/metrics")
def metric_catalog(app: AppDep) -> Any:
    return app.analytics.definitions()


@router.post("/ws/{ws}/analytics/query")
def analytics_query(app: AppDep, ctx: CtxDep, ws: str, body: AnalyticsQuery) -> Any:
    return app.analytics.query(ctx, ws, body)


@router.get("/ws/{ws}/analytics/overview")
def analytics_overview(app: AppDep, ctx: CtxDep, ws: str, period: str = "week") -> Any:
    return app.analytics.overview(ctx, ws, period=period)


class CompareBody(BaseModel):
    metric: str
    a_since: datetime
    a_until: datetime
    b_since: datetime
    b_until: datetime
    group_by: list[str] = Field(default_factory=list)


@router.post("/ws/{ws}/analytics/compare")
def analytics_compare(app: AppDep, ctx: CtxDep, ws: str, body: CompareBody) -> Any:
    return app.analytics.compare(
        ctx,
        ws,
        body.metric,
        a=(body.a_since, body.a_until),
        b=(body.b_since, body.b_until),
        group_by=body.group_by,
    )


@router.get("/ws/{ws}/analytics/coupling")
def analytics_coupling(
    app: AppDep, ctx: CtxDep, ws: str, project: str | None = None, min_support: int = 1
) -> Any:
    return app.analytics.coupling(ctx, ws, project=project, min_support=min_support)


@router.get("/ws/{ws}/analytics/failures")
def analytics_failures(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.analytics.failures(ctx, ws)


@router.get("/ws/{ws}/analytics/history")
def analytics_history(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    category: str | None = None,
    repository: str | None = None,
    limit: int = Query(50, le=500),
) -> Any:
    return app.analytics.task_history(ctx, ws, category=category, repository=repository, limit=limit)


class EstimateBody(BaseModel):
    task: str | None = None
    title: str | None = None
    description: str = ""
    category: str | None = None
    repositories: list[str] = Field(default_factory=list)


@router.post("/ws/{ws}/analytics/estimate")
def analytics_estimate(app: AppDep, ctx: CtxDep, ws: str, body: EstimateBody) -> Any:
    return app.analytics.estimate(
        ctx,
        ws,
        task=body.task,
        title=body.title,
        description=body.description,
        category=body.category,
        repositories=body.repositories,
    )


class ExportBody(BaseModel):
    format: Literal["csv", "json", "parquet"] = "csv"
    dataset: str | None = None
    query: AnalyticsQuery | None = None


@router.post("/ws/{ws}/analytics/export")
def analytics_export(app: AppDep, ctx: CtxDep, ws: str, body: ExportBody) -> Any:
    if not body.dataset and not body.query:
        raise ValidationFailed("provide a dataset or a query")
    name = body.dataset or (body.query.metric if body.query else "export")
    target = Path(tempfile.mkdtemp(prefix="wg-export-")) / f"{name}.{body.format}"
    app.analytics.export(ctx, ws, target=target, fmt=body.format, dataset=body.dataset, query=body.query)
    return FileResponse(target, filename=target.name)


@router.get("/ws/{ws}/reports")
def reports(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    require(ctx, Permission.ANALYTICS_READ)
    from workgraph.analytics.retrospective import list_reports

    return list_reports(app, ws)


class RetroBody(BaseModel):
    period_days: int = Field(default=14, ge=1, le=366)
    project: str | None = None
    repository: str | None = None
    use_model: bool = True


@router.post("/ws/{ws}/reports/retrospective", status_code=201)
def retrospective(app: AppDep, ctx: CtxDep, ws: str, body: RetroBody) -> Any:
    require(ctx, Permission.ANALYTICS_READ)
    from workgraph.analytics.retrospective import generate_retrospective

    return generate_retrospective(
        app,
        ctx,
        ws,
        period_days=body.period_days,
        project=body.project,
        repository=body.repository,
        use_model=body.use_model,
    )


@router.get("/ws/{ws}/search")
def search(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    q: str = Query(..., min_length=1),
    mode: Literal["hybrid", "lexical", "semantic"] = "hybrid",
    project: str | None = None,
    repository: str | None = None,
    category: str | None = None,
    status: list[str] | None = Query(None),
    agent: str | None = None,
    model: str | None = None,
    path: str | None = None,
    created_after: datetime | None = None,
    created_before: datetime | None = None,
    limit: int = Query(20, le=200),
) -> Any:
    return app.search.search(
        ctx,
        ws,
        q,
        mode=mode,
        limit=limit,
        filters=SearchFilters(
            project=project,
            repository=repository,
            category=category,
            statuses=status,
            agent=agent,
            model=model,
            changed_path=path,
            created_after=created_after,
            created_before=created_before,
        ),
    )


@router.get("/ws/{ws}/classification/evaluations")
def evaluations(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.classification.evaluations(ctx, ws)


@router.get("/ws/{ws}/classification/disagreements")
def disagreements(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    return app.classification.disagreements(ctx, ws)


# ---------------------------------------------------------------------------- settings & integrations


@router.get("/settings")
def settings(app: AppDep, ctx: CtxDep, workspace: str | None = None) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    config = app.effective_config(workspace or app.config.default_workspace)
    return {
        "config": config.model_dump(mode="json"),
        "paths": {"config_file": str(app.paths.config_file), "data_dir": str(app.paths.data_dir)},
    }


class SettingsPatch(BaseModel):
    values: dict[str, Any] = Field(description="Dotted keys, e.g. {'execution.max_concurrent_runs': 3}")


EDITABLE_PREFIXES = (
    "execution.",
    "jobs.",
    "llm.classification.",
    "llm.embeddings.",
    "llm.summarization.",
    "llm.auto_classify",
    "llm.auto_embed",
    "retention.",
    "claude.capture_tool_payloads",
    "automation.max_depth",
    "stacks.auto_accept",
    "verification.auto_accept",
)


@router.patch("/settings")
def patch_settings(app: AppDep, ctx: CtxDep, body: SettingsPatch) -> Any:
    require(ctx, Permission.CONFIG_MANAGE)
    from workgraph.config.settings import GlobalConfig

    data = app.config.model_dump(mode="json")
    for key, value in body.values.items():
        if not key.startswith(EDITABLE_PREFIXES):
            raise ValidationFailed(f"{key} cannot be edited from the API; use the config file")
        cursor = data
        parts = key.split(".")
        for part in parts[:-1]:
            cursor = cursor.setdefault(part, {})
        cursor[parts[-1]] = value
    try:
        config = GlobalConfig.model_validate(data)
    except Exception as exc:
        raise ValidationFailed(f"invalid settings: {exc}") from exc
    app.config_store.save_global(config)
    return {"config": config.model_dump(mode="json")}


@router.get("/agents")
def agents(app: AppDep, ctx: CtxDep) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    return app.agents.describe()


@router.get("/llm/health")
def llm_health(app: AppDep, ctx: CtxDep) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    return app.llm.health()


@router.get("/claude/status")
def claude_status(app: AppDep, ctx: CtxDep) -> Any:
    require(ctx, Permission.WORKSPACE_READ)
    from workgraph.integrations.claude_code import installer

    return {
        "user": installer.status("user"),
        "hooks": app.hooks.stats(),
        "sessions": app.hooks.sessions(limit=20),
    }


@router.get("/ws/{ws}/scheduler")
def scheduler(app: AppDep, ctx: CtxDep, ws: str) -> Any:
    require(ctx, Permission.TASK_READ)
    ws_id = app.workspaces.get(ctx, ws).id
    return {
        "enabled": app.config.execution.enabled,
        "auto_schedule": app.config.execution.auto_schedule,
        "eligible": app.scheduler.eligible(ctx, ws_id),
    }


# ---------------------------------------------------------------------------- events


@router.get("/ws/{ws}/events")
def events(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    after_seq: int | None = None,
    entity_type: str | None = None,
    entity_id: str | None = None,
    prefix: str | None = None,
    limit: int = Query(200, le=5000),
    descending: bool = False,
) -> Any:
    return app.history.events(
        ctx,
        ws,
        after_seq=after_seq,
        entity_type=entity_type,
        entity_id=entity_id,
        event_type_prefix=prefix,
        limit=limit,
        descending=descending,
    )


@router.get("/ws/{ws}/events/stream")
async def event_stream(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    request: Request,
    after_seq: int | None = None,
    max_events: int | None = Query(None, ge=1, le=10_000),
) -> Any:
    """Server-sent events: new domain events plus periodic queue snapshots."""
    require(ctx, Permission.WORKSPACE_READ)
    from workgraph.core.events.log import latest_seq

    ws_id = app.workspaces.get(ctx, ws).id
    if after_seq is None:
        with app.db.session() as session:
            after_seq = latest_seq(session)

    async def generator() -> Any:
        cursor = after_seq
        ticks = 0
        sent = 0
        while not await request.is_disconnected():
            batch = await asyncio.to_thread(app.history.events, ctx, ws_id, after_seq=cursor, limit=200)
            for event in batch:
                cursor = event.seq
                yield {"event": "domain", "id": str(event.seq), "data": event.model_dump_json()}
                sent += 1
                if max_events and sent >= max_events:
                    return
            if ticks % 5 == 0:
                stats = await asyncio.to_thread(app.queue.stats, ws_id)
                yield {"event": "queue", "data": json.dumps(stats)}
                sent += 1
                if max_events and sent >= max_events:
                    return
            ticks += 1
            await asyncio.sleep(1.0)

    return EventSourceResponse(generator(), ping=15)
