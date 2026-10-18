"""Experiments: a pinned protocol and the trials measured under it."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import ForeignKey, Integer, String, Text, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class Experiment(Base):
    __tablename__ = "experiments"
    __table_args__ = (UniqueConstraint("workspace_id", "slug"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    slug: Mapped[str] = mapped_column(String(100))
    title: Mapped[str] = mapped_column(String(500))
    question: Mapped[str | None] = mapped_column(Text, default=None)
    status: Mapped[str] = mapped_column(String(16), default="draft")
    # The protocol is validated on every write and frozen once the experiment starts running.
    protocol: Mapped[dict[str, Any]] = mapped_column(default=dict)
    frozen_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class ExperimentTrial(Base):
    """One task, one attempt, one model, one arm of an experiment."""

    __tablename__ = "experiment_trials"
    __table_args__ = (UniqueConstraint("experiment_id", "seq"),)

    id: Mapped[str] = pk()
    experiment_id: Mapped[str] = mapped_column(ForeignKey("experiments.id", ondelete="CASCADE"), index=True)
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="RESTRICT"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None)
    subagent_run_id: Mapped[str | None] = mapped_column(String(36), default=None)
    arm: Mapped[str] = mapped_column(String(64))
    requested_model: Mapped[str] = mapped_column(String(200))
    actual_model: Mapped[str | None] = mapped_column(String(200), default=None)
    phase: Mapped[str] = mapped_column(String(16))
    seq: Mapped[int] = mapped_column(Integer)
    band: Mapped[int | None] = mapped_column(Integer, default=None)
    features: Mapped[dict[str, Any]] = mapped_column(default=dict)
    base_ref: Mapped[str | None] = mapped_column(String(300), default=None)
    worktree_path: Mapped[str | None] = mapped_column(String(1000), default=None)
    status: Mapped[str] = mapped_column(String(16), default="planned")
    outcome: Mapped[str | None] = mapped_column(String(8), default=None)
    outcome_detail: Mapped[dict[str, Any]] = mapped_column(default=dict)
    failure_cause: Mapped[str | None] = mapped_column(String(32), default=None)
    failure_cause_source: Mapped[str | None] = mapped_column(String(16), default=None)
    notes: Mapped[str | None] = mapped_column(Text, default=None)
    created_at: Mapped[datetime] = created_col()
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    graded_at: Mapped[datetime | None] = mapped_column(default=None)
