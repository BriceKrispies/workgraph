"""Agent attempts: first-class records of each execution of a task."""

from __future__ import annotations

import builtins
from typing import Any

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import TERMINAL_ATTEMPT_STATUSES, AttemptStatus, TaskStatus
from workgraph.core.models.schemas import AttemptCreate, AttemptOut, Page
from workgraph.core.policies.lifecycle import check_attempt_transition
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.persistence.orm import AgentAttempt, RevisionManifest, Task
from workgraph.security.permissions import Permission, require

# Usage keys accepted from provider telemetry; anything else is dropped rather than guessed.
USAGE_KEYS = {
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
    "total_tokens",
    "num_turns",
}


def sanitize_usage(usage: dict[str, Any] | None) -> dict[str, Any]:
    if not usage:
        return {}
    clean: dict[str, Any] = {}
    for key, value in usage.items():
        if (
            key in USAGE_KEYS
            and isinstance(value, int | float)
            and not isinstance(value, bool)
            and value >= 0
        ):
            clean[key] = int(value)
    return clean


def next_attempt_number(session: Session, task_id: str) -> int:
    current = session.scalar(select(func.max(AgentAttempt.number)).where(AgentAttempt.task_id == task_id))
    return int(current or 0) + 1


def task_snapshot(task: Task) -> dict[str, Any]:
    return {
        "key": task.key,
        "version": task.version,
        "title": task.title,
        "description": task.description,
        "acceptance_criteria": list(task.acceptance_criteria or []),
        "priority": task.priority,
        "category": task.category,
    }


def new_attempt(
    session: Session,
    ctx: RequestContext,
    task: Task,
    *,
    provider: str,
    adapter: str,
    status: AttemptStatus,
    agent_id: str | None = None,
    model: str | None = None,
    session_id: str | None = None,
    parent_attempt_id: str | None = None,
    retry_of_attempt_id: str | None = None,
    rerun_of_attempt_id: str | None = None,
    config_snapshot: dict[str, Any] | None = None,
    environment: dict[str, Any] | None = None,
) -> AgentAttempt:
    now = utcnow()
    attempt = AgentAttempt(
        workspace_id=task.workspace_id,
        task_id=task.id,
        number=next_attempt_number(session, task.id),
        status=status,
        provider=provider,
        adapter=adapter,
        agent_id=agent_id,
        model=model,
        session_id=session_id,
        parent_attempt_id=parent_attempt_id,
        retry_of_attempt_id=retry_of_attempt_id,
        rerun_of_attempt_id=rerun_of_attempt_id,
        created_by=ctx.actor,
        queued_at=now,
        started_at=now if status == AttemptStatus.RUNNING else None,
        queue_wait_ms=0 if status == AttemptStatus.RUNNING else None,
        input_task_version=task.version,
        input_task_snapshot=task_snapshot(task),
        config_snapshot=config_snapshot or {},
        environment=environment or {},
    )
    session.add(attempt)
    session.flush()
    record_event(
        session,
        ctx,
        event_type="attempt.created",
        entity_type="attempt",
        entity_id=attempt.id,
        workspace_id=task.workspace_id,
        attempt_id=attempt.id,
        data={
            "task_id": task.id,
            "task_key": task.key,
            "number": attempt.number,
            "provider": provider,
            "adapter": adapter,
            "model": model,
            "status": status,
            "parent_attempt_id": parent_attempt_id,
            "retry_of_attempt_id": retry_of_attempt_id,
            "rerun_of_attempt_id": rerun_of_attempt_id,
        },
    )
    return attempt


def set_attempt_status(
    session: Session, ctx: RequestContext, attempt: AgentAttempt, status: AttemptStatus, **data: Any
) -> None:
    current = AttemptStatus(attempt.status)
    if current == status:
        return
    check_attempt_transition(current, status)
    now = utcnow()
    attempt.status = status
    if status == AttemptStatus.RUNNING and attempt.started_at is None:
        attempt.started_at = now
        if attempt.queued_at is not None:
            attempt.queue_wait_ms = max(0, int((now - attempt.queued_at).total_seconds() * 1000))
    record_event(
        session,
        ctx,
        event_type="attempt.status_changed",
        entity_type="attempt",
        entity_id=attempt.id,
        workspace_id=attempt.workspace_id,
        attempt_id=attempt.id,
        data={"from": current, "to": status, "task_id": attempt.task_id, **data},
    )


def finish_attempt(
    session: Session,
    ctx: RequestContext,
    attempt: AgentAttempt,
    status: AttemptStatus,
    *,
    exit_reason: str | None = None,
    exit_code: int | None = None,
    error: str | None = None,
    summary: str | None = None,
    usage: dict[str, Any] | None = None,
    usage_source: str | None = None,
    cost_usd: float | None = None,
    cost_source: str | None = None,
    telemetry: dict[str, Any] | None = None,
    duration_ms: int | None = None,
) -> None:
    if status not in TERMINAL_ATTEMPT_STATUSES:
        raise ValidationFailed(f"{status} is not a terminal attempt status")
    if AttemptStatus(attempt.status) in TERMINAL_ATTEMPT_STATUSES:
        if attempt.status == status:
            return
        raise ValidationFailed(f"attempt {attempt.id} already finished as {attempt.status}")
    check_attempt_transition(AttemptStatus(attempt.status), status)
    now = utcnow()
    previous = attempt.status
    attempt.status = status
    attempt.finished_at = now
    attempt.exit_reason = exit_reason
    attempt.exit_code = exit_code
    if error:
        attempt.error = error[:20_000]
    if summary:
        attempt.summary = summary[:50_000]
    clean_usage = sanitize_usage(usage)
    if clean_usage:
        attempt.usage = clean_usage
        attempt.usage_source = usage_source or "measured"
    if cost_usd is not None:
        attempt.cost_usd = float(cost_usd)
        attempt.cost_source = cost_source or "measured"
    if telemetry:
        attempt.telemetry = {**(attempt.telemetry or {}), **telemetry}
    if duration_ms is not None:
        attempt.duration_ms = duration_ms
    elif attempt.started_at is not None:
        attempt.duration_ms = max(0, int((now - attempt.started_at).total_seconds() * 1000))
    event_type = {
        AttemptStatus.SUCCEEDED: "attempt.succeeded",
        AttemptStatus.FAILED: "attempt.failed",
        AttemptStatus.TIMED_OUT: "attempt.failed",
        AttemptStatus.CANCELLED: "attempt.cancelled",
        AttemptStatus.ABANDONED: "attempt.failed",
    }[status]
    record_event(
        session,
        ctx,
        event_type=event_type,
        entity_type="attempt",
        entity_id=attempt.id,
        workspace_id=attempt.workspace_id,
        attempt_id=attempt.id,
        data={
            "task_id": attempt.task_id,
            "from": previous,
            "status": status,
            "exit_reason": exit_reason,
            "exit_code": exit_code,
            "duration_ms": attempt.duration_ms,
            "error": (error or "")[:1000] or None,
        },
    )


class AttemptService(Service):
    def create(self, ctx: RequestContext, workspace: str | None, data: AttemptCreate) -> AttemptOut:
        """Register an attempt by an externally running agent (e.g. via MCP)."""
        require(ctx, Permission.ATTEMPT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, data.task)
            parent_id = None
            if data.parent_attempt:
                parent = resolve.attempt(session, data.parent_attempt)
                if parent.workspace_id != task.workspace_id:
                    raise ValidationFailed("parent attempt belongs to another workspace")
                parent_id = parent.id
            attempt = new_attempt(
                session,
                ctx,
                task,
                provider=data.provider,
                adapter=data.adapter or data.provider,
                status=AttemptStatus(data.status),
                agent_id=data.agent_id or ctx.actor,
                model=data.model,
                session_id=data.session_id,
                parent_attempt_id=parent_id,
            )
            if data.status == "running" and TaskStatus(task.status) == TaskStatus.CLAIMED:
                self.app.tasks._transition(
                    session, ctx, task, TaskStatus.RUNNING, reason=f"attempt #{attempt.number} started"
                )
                task.claimed_by = task.claimed_by or ctx.actor
            session.flush()
            return AttemptOut.model_validate(attempt)

    def get(self, ctx: RequestContext, attempt_id: str) -> AttemptOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            return AttemptOut.model_validate(resolve.attempt(session, attempt_id))

    def list(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        task: str | None = None,
        statuses: builtins.list[str] | None = None,
        provider: str | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> Page[AttemptOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(AgentAttempt).where(AgentAttempt.workspace_id == ws_id)
            if task:
                stmt = stmt.where(AgentAttempt.task_id == resolve.task(session, ws_id, task).id)
            if statuses:
                stmt = stmt.where(AgentAttempt.status.in_(statuses))
            if provider:
                stmt = stmt.where(AgentAttempt.provider == provider)
            total = int(session.scalar(select(func.count()).select_from(stmt.subquery())) or 0)
            rows = session.scalars(stmt.order_by(AgentAttempt.created_at.desc()).limit(limit).offset(offset))
            items = [AttemptOut.model_validate(a) for a in rows]
            return Page[AttemptOut](
                items=items,
                total=total,
                limit=limit,
                offset=offset,
                next_offset=offset + limit if offset + limit < total else None,
            )

    def finish(
        self,
        ctx: RequestContext,
        attempt_id: str,
        status: AttemptStatus,
        *,
        summary: str | None = None,
        error: str | None = None,
        usage: dict[str, Any] | None = None,
    ) -> AttemptOut:
        require(ctx, Permission.ATTEMPT_WRITE)
        with self.db.session(write=True) as session:
            attempt = resolve.attempt(session, attempt_id)
            finish_attempt(
                session,
                ctx,
                attempt,
                status,
                exit_reason="reported",
                summary=summary,
                error=error,
                usage=usage,
                usage_source="agent_reported" if usage else None,
            )
            session.flush()
            return AttemptOut.model_validate(attempt)

    def manifests(self, ctx: RequestContext, attempt_id: str) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            rows = session.scalars(
                select(RevisionManifest)
                .where(RevisionManifest.attempt_id == attempt_id)
                .order_by(RevisionManifest.created_at)
            )
            return [
                {"id": m.id, "kind": m.kind, "entries": m.entries, "created_at": m.created_at.isoformat()}
                for m in rows
            ]

    def compare(self, ctx: RequestContext, first: str, second: str) -> dict[str, Any]:
        """Side-by-side provenance comparison of two attempts."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            a = resolve.attempt(session, first)
            b = resolve.attempt(session, second)
            fields = [
                "status",
                "provider",
                "adapter",
                "model",
                "input_task_version",
                "duration_ms",
                "queue_wait_ms",
                "exit_reason",
                "usage",
                "cost_usd",
                "modified_repository_ids",
            ]
            diff = {}
            for f in fields:
                va, vb = getattr(a, f), getattr(b, f)
                diff[f] = {"a": va, "b": vb, "same": va == vb}
            config_keys = sorted(set(a.config_snapshot) | set(b.config_snapshot))
            config_diff = {
                k: {"a": a.config_snapshot.get(k), "b": b.config_snapshot.get(k)}
                for k in config_keys
                if a.config_snapshot.get(k) != b.config_snapshot.get(k)
            }
            manifests = {}
            for label, att in (("a", a), ("b", b)):
                manifests[label] = {
                    m.kind: m.entries
                    for m in session.scalars(
                        select(RevisionManifest).where(RevisionManifest.attempt_id == att.id)
                    )
                }
            return {
                "a": a.id,
                "b": b.id,
                "same_task": a.task_id == b.task_id,
                "fields": diff,
                "config_differences": config_diff,
                "input_snapshot_same": a.input_task_snapshot == b.input_task_snapshot,
                "manifests": manifests,
            }
