"""Jobs, automation, verification, classification, search and reports."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import (
    Boolean,
    Float,
    ForeignKey,
    Index,
    Integer,
    LargeBinary,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class Job(Base):
    __tablename__ = "jobs"
    __table_args__ = (Index("ix_jobs_claim", "status", "run_at", "priority"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    kind: Mapped[str] = mapped_column(String(64), index=True)
    status: Mapped[str] = mapped_column(String(16), default="queued")
    priority: Mapped[int] = mapped_column(Integer, default=100)  # lower runs first
    payload: Mapped[dict[str, Any]] = mapped_column(default=dict)
    result: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    run_at: Mapped[datetime] = created_col()
    attempts: Mapped[int] = mapped_column(Integer, default=0)
    max_attempts: Mapped[int] = mapped_column(Integer, default=5)
    backoff_base_seconds: Mapped[float] = mapped_column(Float, default=2.0)
    timeout_seconds: Mapped[float | None] = mapped_column(Float, default=None)
    lease_owner: Mapped[str | None] = mapped_column(String(100), default=None)
    lease_expires_at: Mapped[datetime | None] = mapped_column(default=None)
    heartbeat_at: Mapped[datetime | None] = mapped_column(default=None)
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    finished_at: Mapped[datetime | None] = mapped_column(default=None)
    cancel_requested: Mapped[bool] = mapped_column(Boolean, default=False)
    idempotency_key: Mapped[str | None] = mapped_column(String(300), unique=True, default=None)
    resource_keys: Mapped[list[str]] = mapped_column(default=list)
    last_error: Mapped[str | None] = mapped_column(Text, default=None)
    error_kind: Mapped[str | None] = mapped_column(String(64), default=None)
    correlation_id: Mapped[str | None] = mapped_column(String(36), default=None)
    causation_id: Mapped[str | None] = mapped_column(String(36), default=None)
    task_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class JobResourceClaim(Base):
    """Resource keys held by running jobs; used to enforce concurrency limits atomically."""

    __tablename__ = "job_resource_claims"

    job_id: Mapped[str] = mapped_column(ForeignKey("jobs.id", ondelete="CASCADE"), primary_key=True)
    resource_key: Mapped[str] = mapped_column(String(300), primary_key=True, index=True)


class JobRun(Base):
    __tablename__ = "job_runs"

    id: Mapped[str] = pk()
    job_id: Mapped[str] = mapped_column(ForeignKey("jobs.id", ondelete="CASCADE"), index=True)
    attempt_number: Mapped[int] = mapped_column(Integer)
    worker_id: Mapped[str] = mapped_column(String(100))
    status: Mapped[str] = mapped_column(String(16), default="running")
    error: Mapped[str | None] = mapped_column(Text, default=None)
    started_at: Mapped[datetime] = created_col()
    finished_at: Mapped[datetime | None] = mapped_column(default=None)


class Worker(Base):
    __tablename__ = "workers"

    id: Mapped[str] = mapped_column(String(100), primary_key=True)
    hostname: Mapped[str] = mapped_column(String(200))
    pid: Mapped[int] = mapped_column(Integer)
    status: Mapped[str] = mapped_column(String(16), default="running")
    concurrency: Mapped[int] = mapped_column(Integer, default=1)
    kinds: Mapped[list[str]] = mapped_column(default=list)
    current_jobs: Mapped[list[str]] = mapped_column(default=list)
    jobs_completed: Mapped[int] = mapped_column(Integer, default=0)
    jobs_failed: Mapped[int] = mapped_column(Integer, default=0)
    started_at: Mapped[datetime] = created_col()
    heartbeat_at: Mapped[datetime] = created_col()
    stopped_at: Mapped[datetime | None] = mapped_column(default=None)


class AutomationRule(Base):
    __tablename__ = "automation_rules"
    __table_args__ = (UniqueConstraint("workspace_id", "name"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(200))
    description: Mapped[str] = mapped_column(Text, default="")
    enabled: Mapped[bool] = mapped_column(Boolean, default=True)
    trigger: Mapped[dict[str, Any]] = mapped_column(default=dict)
    conditions: Mapped[dict[str, Any]] = mapped_column(default=dict)
    actions: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    retry_max_attempts: Mapped[int] = mapped_column(Integer, default=3)
    max_depth: Mapped[int] = mapped_column(Integer, default=3)
    last_scheduled_at: Mapped[datetime | None] = mapped_column(default=None)
    next_run_at: Mapped[datetime | None] = mapped_column(default=None)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    version: Mapped[int] = mapped_column(Integer, default=1)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class AutomationRun(Base):
    __tablename__ = "automation_runs"
    __table_args__ = (UniqueConstraint("rule_id", "trigger_key"),)

    id: Mapped[str] = pk()
    rule_id: Mapped[str] = mapped_column(ForeignKey("automation_rules.id", ondelete="CASCADE"), index=True)
    workspace_id: Mapped[str] = mapped_column(String(36), index=True)
    trigger_type: Mapped[str] = mapped_column(String(32))
    trigger_key: Mapped[str] = mapped_column(String(200))  # idempotency per rule
    trigger_event_id: Mapped[str | None] = mapped_column(String(36), default=None)
    depth: Mapped[int] = mapped_column(Integer, default=0)
    dry_run: Mapped[bool] = mapped_column(Boolean, default=False)
    inputs: Mapped[dict[str, Any]] = mapped_column(default=dict)
    status: Mapped[str] = mapped_column(String(16), default="pending")
    matched: Mapped[bool] = mapped_column(Boolean, default=False)
    reason: Mapped[str] = mapped_column(Text, default="")
    condition_trace: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    action_results: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    error: Mapped[str | None] = mapped_column(Text, default=None)
    attempts: Mapped[int] = mapped_column(Integer, default=0)
    correlation_id: Mapped[str | None] = mapped_column(String(36), default=None)
    created_at: Mapped[datetime] = created_col()
    finished_at: Mapped[datetime | None] = mapped_column(default=None)


class VerificationConfig(Base):
    __tablename__ = "verification_configs"
    __table_args__ = (UniqueConstraint("workspace_id", "scope_type", "scope_id", "name"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    scope_type: Mapped[str] = mapped_column(String(16))  # workspace | project | repository | task
    scope_id: Mapped[str] = mapped_column(String(36))
    name: Mapped[str] = mapped_column(String(200))
    checks: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    enabled: Mapped[bool] = mapped_column(Boolean, default=True)
    approved_by: Mapped[str] = mapped_column(String(200))
    version: Mapped[int] = mapped_column(Integer, default=1)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class VerificationRun(Base):
    __tablename__ = "verification_runs"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(String(36), index=True)
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    manifest_id: Mapped[str | None] = mapped_column(String(36), default=None)
    config_snapshot: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    status: Mapped[str] = mapped_column(String(16), default="pending")
    required_passed: Mapped[bool] = mapped_column(Boolean, default=False)
    triggered_by: Mapped[str] = mapped_column(String(200), default="system")
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    finished_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()


class VerificationCheckResult(Base):
    __tablename__ = "verification_check_results"

    id: Mapped[str] = pk()
    run_id: Mapped[str] = mapped_column(ForeignKey("verification_runs.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(200))
    kind: Mapped[str] = mapped_column(String(16))
    required: Mapped[bool] = mapped_column(Boolean, default=True)
    repository_id: Mapped[str | None] = mapped_column(String(36), default=None)
    status: Mapped[str] = mapped_column(String(16))
    exit_code: Mapped[int | None] = mapped_column(Integer, default=None)
    duration_ms: Mapped[int | None] = mapped_column(Integer, default=None)
    timed_out: Mapped[bool] = mapped_column(Boolean, default=False)
    log_artifact_id: Mapped[str | None] = mapped_column(String(36), default=None)
    output_excerpt: Mapped[str] = mapped_column(Text, default="")
    failure_signatures: Mapped[list[str]] = mapped_column(default=list)
    created_at: Mapped[datetime] = created_col()


class TaskAcceptance(Base):
    __tablename__ = "task_acceptances"

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None)
    verification_run_id: Mapped[str | None] = mapped_column(String(36), default=None)
    decision: Mapped[str] = mapped_column(String(16))  # accepted | rejected
    mode: Mapped[str] = mapped_column(String(16))  # automated | manual
    reason: Mapped[str] = mapped_column(Text, default="")
    actor: Mapped[str] = mapped_column(String(200))
    manifest_id: Mapped[str | None] = mapped_column(String(36), default=None)
    created_at: Mapped[datetime] = created_col()


class Classification(Base):
    __tablename__ = "classifications"
    __table_args__ = (Index("ix_classifications_task", "task_id", "created_at"),)

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"))
    source: Mapped[str] = mapped_column(String(16))  # model | manual
    status: Mapped[str] = mapped_column(String(16), default="pending")
    provider: Mapped[str | None] = mapped_column(String(100), default=None)
    model: Mapped[str | None] = mapped_column(String(200), default=None)
    prompt_version: Mapped[str | None] = mapped_column(String(32), default=None)
    taxonomy_version: Mapped[str] = mapped_column(String(32))
    input_hash: Mapped[str] = mapped_column(String(64))
    category: Mapped[str | None] = mapped_column(String(32), default=None)
    subtype: Mapped[str | None] = mapped_column(String(100), default=None)
    domain: Mapped[str | None] = mapped_column(String(100), default=None)
    affected_systems: Mapped[list[str]] = mapped_column(default=list)
    complexity: Mapped[str | None] = mapped_column(String(16), default=None)
    tags: Mapped[list[str]] = mapped_column(default=list)
    confidence: Mapped[float | None] = mapped_column(Float, default=None)
    rationale: Mapped[str | None] = mapped_column(Text, default=None)
    raw_output: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    validation_status: Mapped[str] = mapped_column(String(16), default="pending")
    error: Mapped[str | None] = mapped_column(Text, default=None)
    duration_ms: Mapped[int | None] = mapped_column(Integer, default=None)
    is_current: Mapped[bool] = mapped_column(Boolean, default=False)
    confirmed_by: Mapped[str | None] = mapped_column(String(200), default=None)
    batch_id: Mapped[str | None] = mapped_column(String(36), default=None)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class ClassificationGoldLabel(Base):
    __tablename__ = "classification_gold_labels"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    task_id: Mapped[str | None] = mapped_column(ForeignKey("tasks.id", ondelete="SET NULL"), default=None)
    title: Mapped[str] = mapped_column(String(500))
    description: Mapped[str] = mapped_column(Text, default="")
    category: Mapped[str] = mapped_column(String(32))
    labeled_by: Mapped[str] = mapped_column(String(200))
    created_at: Mapped[datetime] = created_col()


class ClassificationEvaluation(Base):
    __tablename__ = "classification_evaluations"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(String(36), index=True)
    provider: Mapped[str] = mapped_column(String(100))
    model: Mapped[str] = mapped_column(String(200))
    prompt_version: Mapped[str] = mapped_column(String(32))
    taxonomy_version: Mapped[str] = mapped_column(String(32))
    sample_size: Mapped[int] = mapped_column(Integer)
    failures: Mapped[int] = mapped_column(Integer, default=0)
    accuracy: Mapped[float | None] = mapped_column(Float, default=None)
    macro_f1: Mapped[float | None] = mapped_column(Float, default=None)
    per_class: Mapped[dict[str, Any]] = mapped_column(default=dict)
    confusion: Mapped[dict[str, Any]] = mapped_column(default=dict)
    results: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    created_at: Mapped[datetime] = created_col()


class TaskEmbedding(Base):
    __tablename__ = "task_embeddings"
    __table_args__ = (UniqueConstraint("task_id", "provider", "model"),)

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    provider: Mapped[str] = mapped_column(String(100))
    model: Mapped[str] = mapped_column(String(200))
    dimensions: Mapped[int] = mapped_column(Integer)
    vector: Mapped[bytes] = mapped_column(LargeBinary)
    input_hash: Mapped[str] = mapped_column(String(64))
    created_at: Mapped[datetime] = created_col()


class Report(Base):
    __tablename__ = "reports"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    kind: Mapped[str] = mapped_column(String(32))
    title: Mapped[str] = mapped_column(String(300))
    scope: Mapped[dict[str, Any]] = mapped_column(default=dict)
    content_markdown: Mapped[str] = mapped_column(Text, default="")
    data: Mapped[dict[str, Any]] = mapped_column(default=dict)
    narrative_source: Mapped[str] = mapped_column(String(200), default="deterministic")
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class Finding(Base):
    """A persisted, evidence-backed observation from failure-pattern analysis."""

    __tablename__ = "findings"
    __table_args__ = (UniqueConstraint("workspace_id", "fingerprint"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    kind: Mapped[str] = mapped_column(String(64))
    fingerprint: Mapped[str] = mapped_column(String(64))
    title: Mapped[str] = mapped_column(String(500))
    severity: Mapped[str] = mapped_column(String(16), default="info")
    metric_value: Mapped[float | None] = mapped_column(Float, default=None)
    sample_size: Mapped[int] = mapped_column(Integer, default=0)
    evidence: Mapped[dict[str, Any]] = mapped_column(default=dict)
    first_detected_at: Mapped[datetime] = created_col()
    last_detected_at: Mapped[datetime] = updated_col()
