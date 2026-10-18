"""Agent attempts, sessions, telemetry, manifests, worktrees and change sets."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import Boolean, Float, ForeignKey, Index, Integer, String, Text, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class AgentAttempt(Base):
    __tablename__ = "agent_attempts"
    __table_args__ = (
        UniqueConstraint("task_id", "number"),
        Index("ix_attempts_ws_status", "workspace_id", "status"),
    )

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"))
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="RESTRICT"), index=True)
    number: Mapped[int] = mapped_column(Integer)
    status: Mapped[str] = mapped_column(String(16), default="queued")
    provider: Mapped[str] = mapped_column(String(100))
    adapter: Mapped[str] = mapped_column(String(100))
    agent_id: Mapped[str | None] = mapped_column(String(200), default=None)
    model: Mapped[str | None] = mapped_column(String(200), default=None)
    session_id: Mapped[str | None] = mapped_column(String(200), default=None, index=True)
    parent_attempt_id: Mapped[str | None] = mapped_column(
        ForeignKey("agent_attempts.id", ondelete="SET NULL"), default=None
    )
    retry_of_attempt_id: Mapped[str | None] = mapped_column(
        ForeignKey("agent_attempts.id", ondelete="SET NULL"), default=None
    )
    rerun_of_attempt_id: Mapped[str | None] = mapped_column(
        ForeignKey("agent_attempts.id", ondelete="SET NULL"), default=None
    )
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()
    queued_at: Mapped[datetime | None] = mapped_column(default=None)
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    finished_at: Mapped[datetime | None] = mapped_column(default=None)
    queue_wait_ms: Mapped[int | None] = mapped_column(Integer, default=None)
    duration_ms: Mapped[int | None] = mapped_column(Integer, default=None)
    exit_reason: Mapped[str | None] = mapped_column(String(100), default=None)
    exit_code: Mapped[int | None] = mapped_column(Integer, default=None)
    error: Mapped[str | None] = mapped_column(Text, default=None)
    summary: Mapped[str | None] = mapped_column(Text, default=None)
    input_task_version: Mapped[int | None] = mapped_column(Integer, default=None)
    input_task_snapshot: Mapped[dict[str, Any]] = mapped_column(default=dict)
    prompt_artifact_id: Mapped[str | None] = mapped_column(String(36), default=None)
    config_snapshot: Mapped[dict[str, Any]] = mapped_column(default=dict)
    environment: Mapped[dict[str, Any]] = mapped_column(default=dict)
    baseline_manifest_id: Mapped[str | None] = mapped_column(String(36), default=None)
    outcome_manifest_id: Mapped[str | None] = mapped_column(String(36), default=None)
    # Usage is only populated from provider telemetry; ``usage_source`` records provenance.
    usage: Mapped[dict[str, Any]] = mapped_column(default=dict)
    usage_source: Mapped[str | None] = mapped_column(String(32), default=None)
    cost_usd: Mapped[float | None] = mapped_column(Float, default=None)
    cost_source: Mapped[str | None] = mapped_column(String(32), default=None)
    telemetry: Mapped[dict[str, Any]] = mapped_column(default=dict)
    modified_repository_ids: Mapped[list[str]] = mapped_column(default=list)
    retries: Mapped[int] = mapped_column(Integer, default=0)
    cancel_requested: Mapped[bool] = mapped_column(Boolean, default=False)
    updated_at: Mapped[datetime] = updated_col()


class AgentSession(Base):
    """An external agent session (e.g. a Claude Code session) mapped onto Workgraph."""

    __tablename__ = "agent_sessions"
    __table_args__ = (UniqueConstraint("provider", "external_session_id"),)

    id: Mapped[str] = pk()
    provider: Mapped[str] = mapped_column(String(100))
    external_session_id: Mapped[str] = mapped_column(String(200))
    workspace_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    task_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    binding_source: Mapped[str] = mapped_column(String(32), default="unbound")
    cwd: Mapped[str | None] = mapped_column(String(1000), default=None)
    model: Mapped[str | None] = mapped_column(String(200), default=None)
    transcript_path: Mapped[str | None] = mapped_column(String(1000), default=None)
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    ended_at: Mapped[datetime | None] = mapped_column(default=None)
    end_reason: Mapped[str | None] = mapped_column(String(100), default=None)
    tool_calls: Mapped[int] = mapped_column(Integer, default=0)
    tool_failures: Mapped[int] = mapped_column(Integer, default=0)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class SessionBinding(Base):
    """Explicit binding of externally-launched sessions to a task (by session id or cwd)."""

    __tablename__ = "session_bindings"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"))
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    provider: Mapped[str] = mapped_column(String(100), default="claude_code")
    external_session_id: Mapped[str | None] = mapped_column(String(200), default=None, index=True)
    cwd_prefix: Mapped[str | None] = mapped_column(String(1000), default=None)
    active: Mapped[bool] = mapped_column(Boolean, default=True)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class SubagentRun(Base):
    __tablename__ = "subagent_runs"
    __table_args__ = (UniqueConstraint("session_id", "external_agent_id"),)

    id: Mapped[str] = pk()
    session_id: Mapped[str] = mapped_column(ForeignKey("agent_sessions.id", ondelete="CASCADE"), index=True)
    external_agent_id: Mapped[str] = mapped_column(String(200))
    agent_type: Mapped[str | None] = mapped_column(String(200), default=None)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None)
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    stopped_at: Mapped[datetime | None] = mapped_column(default=None)
    last_message_excerpt: Mapped[str | None] = mapped_column(Text, default=None)
    transcript_path: Mapped[str | None] = mapped_column(String(1000), default=None)
    # Measured from the transcript (see integrations/claude_code/transcript.py); never from hooks.
    requested_model: Mapped[str | None] = mapped_column(String(200), default=None)
    model: Mapped[str | None] = mapped_column(String(200), default=None)
    usage: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    telemetry: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    task_id: Mapped[str | None] = mapped_column(
        ForeignKey("tasks.id", ondelete="SET NULL"), index=True, default=None
    )


class AgentInternalTask(Base):
    """A provider-internal task (e.g. Claude Code's task list). Never authoritative."""

    __tablename__ = "agent_internal_tasks"
    __table_args__ = (UniqueConstraint("session_id", "external_task_id"),)

    id: Mapped[str] = pk()
    session_id: Mapped[str] = mapped_column(ForeignKey("agent_sessions.id", ondelete="CASCADE"), index=True)
    external_task_id: Mapped[str] = mapped_column(String(200))
    subject: Mapped[str] = mapped_column(String(500), default="")
    description: Mapped[str | None] = mapped_column(Text, default=None)
    status: Mapped[str] = mapped_column(String(16), default="created")
    task_id: Mapped[str | None] = mapped_column(String(36), default=None)
    external_agent_id: Mapped[str | None] = mapped_column(String(200), default=None)
    created_at: Mapped[datetime] = created_col()
    completed_at: Mapped[datetime | None] = mapped_column(default=None)


class HookEvent(Base):
    """Raw (redacted) ledger of ingested agent lifecycle hook payloads."""

    __tablename__ = "hook_events"

    id: Mapped[str] = pk()
    dedupe_key: Mapped[str] = mapped_column(String(64), unique=True)
    provider: Mapped[str] = mapped_column(String(100), default="claude_code")
    hook_event_name: Mapped[str] = mapped_column(String(64), index=True)
    external_session_id: Mapped[str | None] = mapped_column(String(200), default=None, index=True)
    session_id: Mapped[str | None] = mapped_column(String(36), default=None)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    payload: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    payload_purged_at: Mapped[datetime | None] = mapped_column(default=None)
    received_at: Mapped[datetime] = created_col()
    source: Mapped[str] = mapped_column(String(16), default="direct")  # direct | spool


class ToolEvent(Base):
    __tablename__ = "tool_events"

    id: Mapped[str] = pk()
    hook_event_id: Mapped[str] = mapped_column(ForeignKey("hook_events.id", ondelete="CASCADE"), unique=True)
    session_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    external_agent_id: Mapped[str | None] = mapped_column(String(200), default=None)
    phase: Mapped[str] = mapped_column(String(16))  # pre | success | failure
    tool_name: Mapped[str] = mapped_column(String(200), index=True)
    tool_use_id: Mapped[str | None] = mapped_column(String(200), default=None)
    duration_ms: Mapped[int | None] = mapped_column(Integer, default=None)
    error_excerpt: Mapped[str | None] = mapped_column(Text, default=None)
    summary: Mapped[dict[str, Any]] = mapped_column(default=dict)
    occurred_at: Mapped[datetime] = created_col()


class RevisionManifest(Base):
    __tablename__ = "revision_manifests"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    kind: Mapped[str] = mapped_column(String(32))  # baseline | outcome | verification | accepted
    entries: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    created_at: Mapped[datetime] = created_col()


class ManagedWorktree(Base):
    __tablename__ = "managed_worktrees"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"))
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="RESTRICT"), index=True)
    source_checkout_id: Mapped[str] = mapped_column(
        ForeignKey("repository_checkouts.id", ondelete="RESTRICT"), index=True
    )
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    path: Mapped[str] = mapped_column(String(1000), unique=True)
    branch: Mapped[str] = mapped_column(String(300))
    base_sha: Mapped[str] = mapped_column(String(64))
    status: Mapped[str] = mapped_column(String(16), default="preparing")
    last_error: Mapped[str | None] = mapped_column(Text, default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()
    removed_at: Mapped[datetime | None] = mapped_column(default=None)


class ResourceLease(Base):
    """Exclusive lease over a mutable execution resource (e.g. a user checkout)."""

    __tablename__ = "resource_leases"

    resource_key: Mapped[str] = mapped_column(String(300), primary_key=True)
    holder: Mapped[str] = mapped_column(String(200))
    purpose: Mapped[str] = mapped_column(String(200), default="")
    acquired_at: Mapped[datetime] = created_col()
    expires_at: Mapped[datetime | None] = mapped_column(default=None)


class ChangeSet(Base):
    __tablename__ = "change_sets"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"))
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="RESTRICT"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    title: Mapped[str] = mapped_column(String(500))
    status: Mapped[str] = mapped_column(String(32), default="open")
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class ChangeSetItem(Base):
    __tablename__ = "change_set_items"
    __table_args__ = (UniqueConstraint("change_set_id", "repository_id"),)

    id: Mapped[str] = pk()
    change_set_id: Mapped[str] = mapped_column(ForeignKey("change_sets.id", ondelete="CASCADE"), index=True)
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="RESTRICT"))
    base_sha: Mapped[str | None] = mapped_column(String(64), default=None)
    head_sha: Mapped[str | None] = mapped_column(String(64), default=None)
    branch: Mapped[str | None] = mapped_column(String(300), default=None)
    commit_shas: Mapped[list[str]] = mapped_column(default=list)
    status: Mapped[str] = mapped_column(String(16), default="pending")
    detail: Mapped[str | None] = mapped_column(Text, default=None)
    updated_at: Mapped[datetime] = updated_col()
