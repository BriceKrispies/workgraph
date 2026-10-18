"""Ingestion of Claude Code lifecycle hook payloads.

Claude Code's identifiers (session ids, internal task ids, subagent ids) are mapped
onto Workgraph entities but never become authoritative: a session is bound to an
attempt only through Workgraph's own launch context (environment variables or the
``--session-id`` it chose) or an explicit operator binding.

Ingestion is idempotent: each received payload has a stable ``dedupe_key`` and a
second delivery (direct + spool replay, or a repeated replay) is a no-op.
"""

from __future__ import annotations

import hashlib
import json
import re
import sqlite3
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any

from pydantic import BaseModel, ConfigDict
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from workgraph.config.paths import AppPaths
from workgraph.config.settings import ClaudeIntegrationConfig, ConfigStore
from workgraph.core.clock import ensure_utc, utcnow
from workgraph.core.context import RequestContext
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import TERMINAL_ATTEMPT_STATUSES, AttemptStatus
from workgraph.integrations.claude_code.transcript import TranscriptStats, parse_transcript
from workgraph.persistence.db import Database
from workgraph.persistence.orm import (
    AgentAttempt,
    AgentInternalTask,
    AgentSession,
    HookEvent,
    SessionBinding,
    SubagentRun,
    Task,
    ToolEvent,
)
from workgraph.persistence.schema_version import SCHEMA_HEAD
from workgraph.security.redaction import redact, redact_text, summarize

if TYPE_CHECKING:
    from workgraph.app import Workgraph

PROVIDER = "claude_code"
KNOWN_EVENTS = {
    "SessionStart",
    "SessionEnd",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
    "UserPromptSubmit",
    "Notification",
    "PreCompact",
}
TASK_KEY_RE = re.compile(r"\b([A-Z][A-Z0-9]{0,9}-\d+)\b")
TRANSCRIPT_USAGE_SOURCE = "claude_code_transcript"
SUBAGENT_ADAPTER = "claude_code_subagent"


class HookPayload(BaseModel):
    """Common fields of every hook input. Unknown/new fields are tolerated."""

    model_config = ConfigDict(extra="allow")

    hook_event_name: str
    session_id: str | None = None
    transcript_path: str | None = None
    cwd: str | None = None
    permission_mode: str | None = None
    prompt_id: str | None = None
    agent_id: str | None = None
    agent_type: str | None = None
    # event-specific (all optional, validated loosely for forward compatibility)
    source: str | None = None
    reason: str | None = None
    model: str | Any | None = None
    tool_name: str | None = None
    tool_use_id: str | None = None
    tool_input: Any = None
    tool_response: Any = None
    error: str | None = None
    is_interrupt: bool | None = None
    duration_ms: int | None = None
    agent_transcript_path: str | None = None
    last_assistant_message: str | None = None
    stop_hook_active: bool | None = None
    task_id: str | None = None
    task_subject: str | None = None
    task_description: str | None = None
    teammate_name: str | None = None


def schema_version(payload: dict[str, Any]) -> int:
    """v2: newer Claude Code payloads (prompt ids / effort); v1: original field set."""
    return 2 if ("prompt_id" in payload or "effort" in payload) else 1


def dedupe_key(record: dict[str, Any]) -> str:
    canonical = (
        json.dumps(record.get("payload"), sort_keys=True, default=str) + "|" + str(record.get("received_at"))
    )
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


@dataclass
class IngestResult:
    status: str  # ingested | duplicate | ignored
    hook_event_id: str | None = None
    session_id: str | None = None
    attempt_id: str | None = None


def check_schema(db_path: Path) -> None:
    conn = sqlite3.connect(db_path, timeout=2)
    try:
        row = conn.execute("SELECT version_num FROM alembic_version").fetchone()
    finally:
        conn.close()
    if row is None or row[0] != SCHEMA_HEAD:
        raise RuntimeError(f"database schema {row[0] if row else None} != expected {SCHEMA_HEAD}")


def ingest_direct(paths: AppPaths, record: dict[str, Any]) -> IngestResult:
    """Called from the hook process: minimal setup, short timeouts, raise on any problem."""
    if not paths.database_file.exists():
        raise RuntimeError("Workgraph database not initialised")
    check_schema(paths.database_file)
    config = ConfigStore(paths).load_global().claude
    db = Database(paths.database_file)
    try:
        return process_record(db, config, record, source="direct")
    finally:
        db.dispose()


def _sanitize(payload: dict[str, Any], config: ClaudeIntegrationConfig) -> dict[str, Any]:
    clean = dict(payload)
    for key in ("tool_input", "tool_response"):
        if key in clean:
            clean[key] = (
                redact(clean[key], max_chars=config.max_payload_chars)
                if config.capture_tool_payloads
                else {"summary": summarize(clean[key])}
            )
    if "prompt" in clean:
        clean["prompt"] = (
            redact_text(str(clean["prompt"]), config.max_payload_chars)
            if config.capture_tool_payloads
            else {"length": len(str(clean["prompt"]))}
        )
    for key in ("last_assistant_message", "task_description", "error"):
        if isinstance(clean.get(key), str):
            clean[key] = redact_text(clean[key], 2000)
    return redact(clean, max_chars=max(config.max_payload_chars, 2000))


def _tool_summary(payload: HookPayload) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    tool_input = payload.tool_input if isinstance(payload.tool_input, dict) else {}
    if payload.tool_name in ("Edit", "Write", "MultiEdit", "NotebookEdit", "Read") and tool_input.get(
        "file_path"
    ):
        summary["file_path"] = str(tool_input["file_path"])[:500]
    if payload.tool_name == "Bash" and tool_input.get("command"):
        command = redact_text(str(tool_input["command"]), 300)
        summary["command_head"] = command.split()[0] if command.split() else ""
    if payload.tool_name and payload.tool_name.startswith("mcp__"):
        summary["mcp_server"] = payload.tool_name.split("__")[1] if "__" in payload.tool_name else None
    return summary


def _resolve_binding(
    session: Session, payload: HookPayload, env: dict[str, Any]
) -> tuple[str | None, str | None, str | None, str]:
    """Return (workspace_id, attempt_id, task_id, binding_source)."""
    attempt_id = env.get("attempt_id")
    if attempt_id:
        attempt = session.get(AgentAttempt, attempt_id)
        if attempt is not None:
            return attempt.workspace_id, attempt.id, attempt.task_id, "workgraph_launched"
    if payload.session_id:
        binding = session.scalar(
            select(SessionBinding).where(
                SessionBinding.external_session_id == payload.session_id, SessionBinding.active.is_(True)
            )
        )
        if binding is not None:
            return binding.workspace_id, None, binding.task_id, "explicit_binding"
    if payload.cwd:
        cwd = str(Path(payload.cwd)).lower()
        for binding in session.scalars(
            select(SessionBinding).where(
                SessionBinding.cwd_prefix.is_not(None), SessionBinding.active.is_(True)
            )
        ):
            prefix = str(Path(binding.cwd_prefix or "")).lower()
            if prefix and (
                cwd == prefix or cwd.startswith(prefix.rstrip("\\/") + ("\\" if "\\" in prefix else "/"))
            ):
                return binding.workspace_id, None, binding.task_id, "explicit_binding"
    task_id = env.get("task_id")
    if task_id:
        task = session.get(Task, task_id)
        if task is not None:
            return task.workspace_id, None, task.id, "environment"
    return None, None, None, "unbound"


def process_record(
    db: Database, config: ClaudeIntegrationConfig, record: dict[str, Any], *, source: str
) -> IngestResult:
    raw = record.get("payload") or {}
    if not isinstance(raw, dict) or "hook_event_name" not in raw:
        raw = {"hook_event_name": "Invalid", "original": raw}
    key = dedupe_key(record)
    payload = HookPayload.model_validate(raw)
    sanitized = _sanitize(raw, config)
    sanitized["_workgraph"] = {"schema_version": schema_version(raw), "source": source}
    received = record.get("received_at")
    received_at = ensure_utc(datetime.fromisoformat(received)) if isinstance(received, str) else utcnow()
    env = record.get("env") or {}
    ctx = RequestContext.system("claude-hooks")
    with db.session(write=True) as session:
        if session.scalar(select(HookEvent.id).where(HookEvent.dedupe_key == key)) is not None:
            return IngestResult("duplicate")
        agent_session = None
        attempt_id = None
        if payload.session_id:
            agent_session = session.scalar(
                select(AgentSession).where(
                    AgentSession.provider == PROVIDER, AgentSession.external_session_id == payload.session_id
                )
            )
            if agent_session is None:
                ws_id, attempt_id, task_id, binding = _resolve_binding(session, payload, env)
                agent_session = AgentSession(
                    provider=PROVIDER,
                    external_session_id=payload.session_id,
                    workspace_id=ws_id,
                    attempt_id=attempt_id,
                    task_id=task_id,
                    binding_source=binding,
                    cwd=payload.cwd,
                    transcript_path=payload.transcript_path,
                    started_at=received_at,
                )
                session.add(agent_session)
                session.flush()
            elif agent_session.binding_source == "unbound":
                ws_id, bound_attempt, task_id, binding = _resolve_binding(session, payload, env)
                if binding != "unbound":
                    agent_session.workspace_id, agent_session.attempt_id = ws_id, bound_attempt
                    agent_session.task_id, agent_session.binding_source = task_id, binding
            attempt_id = agent_session.attempt_id
        hook = HookEvent(
            dedupe_key=key,
            provider=PROVIDER,
            hook_event_name=payload.hook_event_name[:64],
            external_session_id=payload.session_id,
            session_id=agent_session.id if agent_session else None,
            attempt_id=attempt_id,
            payload=sanitized,
            received_at=received_at,
            source=source,
        )
        session.add(hook)
        try:
            session.flush()
        except IntegrityError:
            session.rollback()
            return IngestResult("duplicate")
        _apply(session, ctx, payload, hook, agent_session, received_at)
        return IngestResult("ingested", hook.id, agent_session.id if agent_session else None, attempt_id)


def _apply(
    session: Session,
    ctx: RequestContext,
    payload: HookPayload,
    hook: HookEvent,
    agent_session: AgentSession | None,
    at: datetime,
) -> None:
    name = payload.hook_event_name
    ws_id = agent_session.workspace_id if agent_session else None
    attempt_id = agent_session.attempt_id if agent_session else None
    entity_id = agent_session.id if agent_session else hook.id
    if name in ("PreToolUse", "PostToolUse", "PostToolUseFailure") and payload.tool_name:
        phase = {"PreToolUse": "pre", "PostToolUse": "success", "PostToolUseFailure": "failure"}[name]
        session.add(
            ToolEvent(
                hook_event_id=hook.id,
                session_id=agent_session.id if agent_session else None,
                attempt_id=attempt_id,
                external_agent_id=payload.agent_id,
                phase=phase,
                tool_name=payload.tool_name[:200],
                tool_use_id=payload.tool_use_id,
                duration_ms=payload.duration_ms,
                error_excerpt=redact_text(payload.error, 1000) if payload.error else None,
                summary={**_tool_summary(payload), "is_interrupt": payload.is_interrupt},
                occurred_at=at,
            )
        )
        if agent_session is not None and phase != "pre":
            agent_session.tool_calls += 1
            if phase == "failure":
                agent_session.tool_failures += 1
        return
    if agent_session is None:
        return
    if name == "SessionStart":
        agent_session.started_at = agent_session.started_at or at
        agent_session.cwd = payload.cwd or agent_session.cwd
        if isinstance(payload.model, str):
            agent_session.model = payload.model
        record_event(
            session,
            ctx,
            event_type="agent_session.started",
            entity_type="agent_session",
            entity_id=entity_id,
            workspace_id=ws_id,
            attempt_id=attempt_id,
            data={
                "provider": PROVIDER,
                "external_session_id": payload.session_id,
                "source": payload.source,
                "binding": agent_session.binding_source,
                "task_id": agent_session.task_id,
            },
        )
    elif name == "SessionEnd":
        agent_session.ended_at = at
        agent_session.end_reason = payload.reason
        _roll_up(session, agent_session)
        record_event(
            session,
            ctx,
            event_type="agent_session.ended",
            entity_type="agent_session",
            entity_id=entity_id,
            workspace_id=ws_id,
            attempt_id=attempt_id,
            data={
                "reason": payload.reason,
                "tool_calls": agent_session.tool_calls,
                "tool_failures": agent_session.tool_failures,
            },
        )
    elif name in ("SubagentStart", "SubagentStop") and payload.agent_id:
        run = session.scalar(
            select(SubagentRun).where(
                SubagentRun.session_id == agent_session.id, SubagentRun.external_agent_id == payload.agent_id
            )
        )
        if run is None:
            run = SubagentRun(
                session_id=agent_session.id,
                external_agent_id=payload.agent_id,
                agent_type=payload.agent_type,
                started_at=at if name == "SubagentStart" else None,
            )
            session.add(run)
            session.flush()
            if attempt_id:
                run.attempt_id = _child_attempt(session, ctx, attempt_id, payload, at)
        if name == "SubagentStop":
            run.stopped_at = at
            run.transcript_path = payload.agent_transcript_path
            run.last_message_excerpt = redact_text(payload.last_assistant_message or "", 1000) or None
            _apply_transcript(
                session,
                ctx,
                run,
                external_session_id=agent_session.external_session_id,
                workspace_id=agent_session.workspace_id,
                at=at,
            )
            if run.attempt_id:
                child = session.get(AgentAttempt, run.attempt_id)
                if child is not None and AttemptStatus(child.status) not in TERMINAL_ATTEMPT_STATUSES:
                    from workgraph.core.services.attempts import finish_attempt

                    finish_attempt(
                        session,
                        ctx,
                        child,
                        AttemptStatus.SUCCEEDED,
                        exit_reason="subagent_stopped",
                        summary=run.last_message_excerpt,
                    )
        record_event(
            session,
            ctx,
            event_type=f"agent_session.{'subagent_started' if name == 'SubagentStart' else 'subagent_stopped'}",
            entity_type="agent_session",
            entity_id=entity_id,
            workspace_id=ws_id,
            attempt_id=attempt_id,
            data={
                "agent_id": payload.agent_id,
                "agent_type": payload.agent_type,
                "child_attempt_id": run.attempt_id,
            },
        )
    elif name in ("TaskCreated", "TaskCompleted") and payload.task_id:
        internal = session.scalar(
            select(AgentInternalTask).where(
                AgentInternalTask.session_id == agent_session.id,
                AgentInternalTask.external_task_id == payload.task_id,
            )
        )
        if internal is None:
            internal = AgentInternalTask(
                session_id=agent_session.id,
                external_task_id=payload.task_id,
                subject=(payload.task_subject or "")[:500],
                description=redact_text(payload.task_description or "", 2000) or None,
                external_agent_id=payload.agent_id,
            )
            session.add(internal)
        mapped = _map_internal_task(session, ws_id, payload)
        if mapped:
            internal.task_id = mapped
        if name == "TaskCompleted":
            internal.status = "completed"
            internal.completed_at = at
        session.flush()
        record_event(
            session,
            ctx,
            event_type="agent_session.internal_task_"
            + ("completed" if name == "TaskCompleted" else "created"),
            entity_type="agent_session",
            entity_id=entity_id,
            workspace_id=ws_id,
            attempt_id=attempt_id,
            data={
                "external_task_id": payload.task_id,
                "subject": internal.subject,
                "mapped_task_id": internal.task_id,
                "authoritative": False,
            },
        )
    elif name == "Stop":
        _roll_up(session, agent_session)


def _map_internal_task(session: Session, ws_id: str | None, payload: HookPayload) -> str | None:
    """Link an internal task to a Workgraph task only when it names one explicitly."""
    if not ws_id:
        return None
    text = f"{payload.task_subject or ''} {payload.task_description or ''}"
    for key in TASK_KEY_RE.findall(text):
        task = session.scalar(select(Task).where(Task.workspace_id == ws_id, Task.key == key))
        if task is not None:
            return task.id
    return None


def _child_attempt(
    session: Session, ctx: RequestContext, parent_id: str, payload: HookPayload, at: datetime
) -> str | None:
    parent = session.get(AgentAttempt, parent_id)
    if parent is None:
        return None
    task = session.get(Task, parent.task_id)
    if task is None:
        return None
    from workgraph.core.services.attempts import new_attempt

    child = new_attempt(
        session,
        ctx,
        task,
        provider=PROVIDER,
        adapter="claude_code_subagent",
        status=AttemptStatus.RUNNING,
        agent_id=f"subagent:{payload.agent_type or 'unknown'}",
        session_id=payload.session_id,
        parent_attempt_id=parent.id,
    )
    child.started_at = at
    child.telemetry = {"external_agent_id": payload.agent_id, "agent_type": payload.agent_type}
    return child.id


@dataclass
class TranscriptApplied:
    parsed: bool = False
    linked: bool = False
    attempt_created: bool = False


def _apply_transcript(
    session: Session,
    ctx: RequestContext,
    run: SubagentRun,
    *,
    external_session_id: str | None,
    workspace_id: str | None,
    at: datetime,
) -> TranscriptApplied:
    """Fill a subagent run from its transcript. Shared by the SubagentStop hook and the backfill.

    Transcript parsing is advisory: a missing or unreadable file leaves the run untouched.
    A run with a child attempt gets that attempt's model and usage. A run without one but with a
    linked task gets a finished top-level attempt, so the subagent's work is still accounted for.
    """
    result = TranscriptApplied()
    if not run.transcript_path:
        return result
    stats = _parse_transcript_quietly(run.transcript_path)
    if stats is None:
        return result
    result.parsed = True
    telemetry = stats.telemetry()
    run.requested_model = stats.requested_model
    run.model = stats.model
    run.telemetry = telemetry
    usage = stats.usage if stats.turns else None
    if usage is not None:
        run.usage = usage
    task = _transcript_task(session, workspace_id, stats.task_keys)
    if task is not None:
        run.task_id = task.id
        result.linked = True
    if run.attempt_id:
        child = session.get(AgentAttempt, run.attempt_id)
        if child is not None:
            child.model = stats.model
            if usage is not None:
                child.usage = usage
                child.usage_source = TRANSCRIPT_USAGE_SOURCE
    elif task is not None:
        from workgraph.core.services.attempts import finish_attempt, new_attempt

        attempt = new_attempt(
            session,
            ctx,
            task,
            provider=PROVIDER,
            adapter=SUBAGENT_ADAPTER,
            status=AttemptStatus.RUNNING,
            agent_id=f"subagent:{stats.agent_type or run.agent_type or 'unknown'}",
            model=stats.model,
            session_id=external_session_id,
        )
        attempt.started_at = _parse_timestamp(stats.started_at) or run.started_at or run.stopped_at or at
        run.attempt_id = attempt.id
        result.attempt_created = True
        finish_attempt(
            session,
            ctx,
            attempt,
            AttemptStatus.SUCCEEDED,
            exit_reason="subagent_stopped",
            summary=run.last_message_excerpt,
            usage=usage,
            usage_source=TRANSCRIPT_USAGE_SOURCE if usage is not None else None,
            telemetry=telemetry,
        )
    return result


def _parse_transcript_quietly(path: str) -> TranscriptStats | None:
    try:
        return parse_transcript(path)
    except Exception:  # a parse failure must never break ingestion
        return None


def _transcript_task(session: Session, workspace_id: str | None, keys: tuple[str, ...]) -> Task | None:
    """The task named by the prompt's first ``Task-ID:`` line, only if exactly one task in scope has that key.

    Only the first marker counts: a brief leads with its own marker, and text quoted further down (a replayed
    commit message, for one) can carry other ``Task-ID:`` trailers.
    """
    if not keys:
        return None
    stmt = select(Task).where(Task.key == keys[0])
    if workspace_id:
        stmt = stmt.where(Task.workspace_id == workspace_id)
    matches = list(session.scalars(stmt))
    return matches[0] if len(matches) == 1 else None


def _parse_timestamp(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return ensure_utc(datetime.fromisoformat(value))
    except ValueError:
        return None


def _roll_up(session: Session, agent_session: AgentSession) -> None:
    if not agent_session.attempt_id:
        return
    attempt = session.get(AgentAttempt, agent_session.attempt_id)
    if attempt is None:
        return
    telemetry = dict(attempt.telemetry or {})
    telemetry["hook_tool_calls"] = agent_session.tool_calls
    telemetry["hook_tool_failures"] = agent_session.tool_failures
    telemetry["hook_subagents"] = len(
        list(session.scalars(select(SubagentRun.id).where(SubagentRun.session_id == agent_session.id)))
    )
    attempt.telemetry = telemetry


class HookIngestor:
    """Application-level access: replay the spool, bind sessions, inspect sessions."""

    def __init__(self, app: Workgraph) -> None:
        self.app = app

    def ingest(self, record: dict[str, Any], source: str = "direct") -> IngestResult:
        return process_record(self.app.db, self.app.config.claude, record, source=source)

    def backfill_transcripts(self, ctx: RequestContext, limit: int | None = None) -> dict[str, int]:
        """Apply transcript telemetry to subagent runs that do not have a model yet (idempotent)."""
        from workgraph.security.permissions import Permission, require

        require(ctx, Permission.TELEMETRY_INGEST)
        counts = {"parsed": 0, "missing": 0, "linked": 0, "attempts_created": 0}
        with self.app.db.session(write=True) as session:
            stmt = (
                select(SubagentRun)
                .where(SubagentRun.transcript_path.is_not(None), SubagentRun.model.is_(None))
                .order_by(SubagentRun.id)
            )
            if limit is not None:
                stmt = stmt.limit(limit)
            for run in list(session.scalars(stmt)):
                agent_session = session.get(AgentSession, run.session_id)
                result = _apply_transcript(
                    session,
                    ctx,
                    run,
                    external_session_id=agent_session.external_session_id if agent_session else None,
                    workspace_id=agent_session.workspace_id if agent_session else None,
                    at=utcnow(),
                )
                counts["parsed" if result.parsed else "missing"] += 1
                counts["linked"] += int(result.linked)
                counts["attempts_created"] += int(result.attempt_created)
        return counts

    def replay_spool(self, limit: int = 500) -> dict[str, int]:
        """Ingest spooled hook records; a record failing five times is set aside as ``.failed``."""
        from workgraph.core.fsutil import atomic_write_text

        counts = {"ingested": 0, "duplicate": 0, "failed": 0}
        for file in sorted(self.app.paths.spool_dir.glob("*.json"))[:limit]:
            try:
                record = json.loads(file.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                counts["failed"] += 1
                file.replace(file.with_suffix(".failed"))
                continue
            try:
                result = self.ingest(record, source="spool")
            except Exception as exc:
                counts["failed"] += 1
                record["replay_attempts"] = int(record.get("replay_attempts", 0)) + 1
                record["last_replay_error"] = f"{type(exc).__name__}: {exc}"[:500]
                if record["replay_attempts"] >= 5:
                    atomic_write_text(file.with_suffix(".failed"), json.dumps(record, default=str))
                    file.unlink(missing_ok=True)
                else:
                    atomic_write_text(file, json.dumps(record, default=str))
                continue
            counts["duplicate" if result.status == "duplicate" else "ingested"] += 1
            file.unlink(missing_ok=True)
        return counts

    def bind(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        session_id: str | None = None,
        cwd: str | None = None,
    ) -> dict[str, Any]:
        from workgraph.core.services import resolve
        from workgraph.security.permissions import Permission, require

        require(ctx, Permission.TELEMETRY_INGEST)
        if not session_id and not cwd:
            cwd = str(Path.cwd())
        with self.app.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            task = resolve.task(session, ws_id, task_ref)
            binding = SessionBinding(
                workspace_id=ws_id,
                task_id=task.id,
                external_session_id=session_id,
                cwd_prefix=str(Path(cwd).resolve()) if cwd else None,
                created_by=ctx.actor,
            )
            session.add(binding)
            if session_id:
                existing = session.scalar(
                    select(AgentSession).where(
                        AgentSession.provider == PROVIDER, AgentSession.external_session_id == session_id
                    )
                )
                if existing is not None:
                    existing.task_id, existing.workspace_id = task.id, ws_id
                    existing.binding_source = "explicit_binding"
            session.flush()
            record_event(
                session,
                ctx,
                event_type="agent_session.bound",
                entity_type="task",
                entity_id=task.id,
                workspace_id=ws_id,
                data={"session_id": session_id, "cwd": binding.cwd_prefix},
            )
            return {
                "binding_id": binding.id,
                "task": task.key,
                "session_id": session_id,
                "cwd": binding.cwd_prefix,
            }

    def unbind(self, ctx: RequestContext, binding_id: str | None = None, cwd: str | None = None) -> int:
        with self.app.db.session(write=True) as session:
            stmt = select(SessionBinding).where(SessionBinding.active.is_(True))
            if binding_id:
                stmt = stmt.where(SessionBinding.id == binding_id)
            elif cwd:
                stmt = stmt.where(SessionBinding.cwd_prefix == str(Path(cwd).resolve()))
            rows = list(session.scalars(stmt))
            for row in rows:
                row.active = False
            return len(rows)

    def sessions(self, workspace_id: str | None = None, limit: int = 50) -> list[dict[str, Any]]:
        with self.app.db.session() as session:
            stmt = select(AgentSession).where(AgentSession.provider == PROVIDER)
            if workspace_id:
                stmt = stmt.where(AgentSession.workspace_id == workspace_id)
            rows = session.scalars(stmt.order_by(AgentSession.created_at.desc()).limit(limit))
            return [
                {
                    "id": s.id,
                    "external_session_id": s.external_session_id,
                    "binding": s.binding_source,
                    "task_id": s.task_id,
                    "attempt_id": s.attempt_id,
                    "cwd": s.cwd,
                    "model": s.model,
                    "tool_calls": s.tool_calls,
                    "tool_failures": s.tool_failures,
                    "started_at": s.started_at.isoformat() if s.started_at else None,
                    "ended_at": s.ended_at.isoformat() if s.ended_at else None,
                    "end_reason": s.end_reason,
                }
                for s in rows
            ]

    def stats(self) -> dict[str, Any]:
        with self.app.db.session() as session:
            from sqlalchemy import func

            total = session.scalar(select(func.count()).select_from(HookEvent)) or 0
            last = session.scalar(select(func.max(HookEvent.received_at)))
            by_event = {
                name: int(count)
                for name, count in session.execute(
                    select(HookEvent.hook_event_name, func.count()).group_by(HookEvent.hook_event_name)
                )
            }
        return {
            "hook_events": int(total),
            "last_received_at": last.isoformat() if last else None,
            "by_event": by_event,
            "spooled": len(list(self.app.paths.spool_dir.glob("*.json"))),
            "spool_failed": len(list(self.app.paths.spool_dir.glob("*.failed"))),
        }
