"""Workspaces, projects, repositories, tasks and the append-only event log."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import (
    Boolean,
    CheckConstraint,
    ForeignKey,
    Index,
    Integer,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.orm import Mapped, mapped_column, relationship

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class Workspace(Base):
    __tablename__ = "workspaces"

    id: Mapped[str] = pk()
    slug: Mapped[str] = mapped_column(String(64), unique=True)
    name: Mapped[str] = mapped_column(String(200))
    description: Mapped[str] = mapped_column(Text, default="")
    key_prefix: Mapped[str] = mapped_column(String(12))
    next_task_number: Mapped[int] = mapped_column(Integer, default=1)
    settings: Mapped[dict[str, Any]] = mapped_column(default=dict)
    archived_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class Project(Base):
    __tablename__ = "projects"
    __table_args__ = (UniqueConstraint("workspace_id", "slug"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    slug: Mapped[str] = mapped_column(String(64))
    name: Mapped[str] = mapped_column(String(200))
    description: Mapped[str] = mapped_column(Text, default="")
    status: Mapped[str] = mapped_column(String(16), default="active")
    settings: Mapped[dict[str, Any]] = mapped_column(default=dict)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class Repository(Base):
    """Canonical repository identity, independent of path or remote URL."""

    __tablename__ = "repositories"
    __table_args__ = (UniqueConstraint("workspace_id", "name"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    name: Mapped[str] = mapped_column(String(200))
    description: Mapped[str] = mapped_column(Text, default="")
    # Sorted root-commit SHAs; recognises the same history at a new path or remote.
    identity_fingerprint: Mapped[str | None] = mapped_column(String(400), index=True, default=None)
    default_branch: Mapped[str | None] = mapped_column(String(200), default=None)
    tracked_branches: Mapped[list[str]] = mapped_column(default=list)
    access_mode: Mapped[str] = mapped_column(String(16), default="read_write")
    health_status: Mapped[str] = mapped_column(String(16), default="unknown")
    health_detail: Mapped[dict[str, Any]] = mapped_column(default=dict)
    health_checked_at: Mapped[datetime | None] = mapped_column(default=None)
    is_shallow: Mapped[bool] = mapped_column(Boolean, default=False)
    last_scanned_at: Mapped[datetime | None] = mapped_column(default=None)
    archived_at: Mapped[datetime | None] = mapped_column(default=None)
    metadata_: Mapped[dict[str, Any]] = mapped_column("metadata", default=dict)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()

    remotes: Mapped[list[RepositoryRemote]] = relationship(
        back_populates="repository", cascade="all, delete-orphan", order_by="RepositoryRemote.name"
    )
    checkouts: Mapped[list[RepositoryCheckout]] = relationship(
        back_populates="repository", cascade="all, delete-orphan", order_by="RepositoryCheckout.created_at"
    )


class RepositoryRemote(Base):
    __tablename__ = "repository_remotes"
    __table_args__ = (UniqueConstraint("repository_id", "name"),)

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(200))
    url: Mapped[str] = mapped_column(String(1000))
    normalized_url: Mapped[str] = mapped_column(String(1000), index=True)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()

    repository: Mapped[Repository] = relationship(back_populates="remotes")


class RepositoryCheckout(Base):
    __tablename__ = "repository_checkouts"
    __table_args__ = (UniqueConstraint("repository_id", "path"),)

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"), index=True)
    path: Mapped[str] = mapped_column(String(1000), index=True)
    kind: Mapped[str] = mapped_column(String(32), default="clone")
    is_primary: Mapped[bool] = mapped_column(Boolean, default=False)
    access_mode: Mapped[str | None] = mapped_column(String(16), default=None)
    available: Mapped[bool] = mapped_column(Boolean, default=True)
    last_seen_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()

    repository: Mapped[Repository] = relationship(back_populates="checkouts")


class ProjectRepository(Base):
    __tablename__ = "project_repositories"

    project_id: Mapped[str] = mapped_column(ForeignKey("projects.id", ondelete="CASCADE"), primary_key=True)
    repository_id: Mapped[str] = mapped_column(
        ForeignKey("repositories.id", ondelete="CASCADE"), primary_key=True, index=True
    )
    role: Mapped[str] = mapped_column(String(32), default="member")
    access_mode: Mapped[str | None] = mapped_column(String(16), default=None)
    created_at: Mapped[datetime] = created_col()


class RepositoryDependency(Base):
    """A *declared* relationship between repositories (as opposed to observed co-change)."""

    __tablename__ = "repository_dependencies"
    __table_args__ = (
        UniqueConstraint("from_repository_id", "to_repository_id"),
        CheckConstraint("from_repository_id <> to_repository_id", name="no_self"),
    )

    id: Mapped[str] = pk()
    from_repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"))
    to_repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"))
    kind: Mapped[str] = mapped_column(String(32), default="depends_on")
    description: Mapped[str] = mapped_column(Text, default="")
    created_at: Mapped[datetime] = created_col()


class Task(Base):
    __tablename__ = "tasks"
    __table_args__ = (
        UniqueConstraint("workspace_id", "number"),
        UniqueConstraint("workspace_id", "key"),
        Index("ix_tasks_ws_status", "workspace_id", "status"),
        CheckConstraint("parent_id IS NULL OR parent_id <> id", name="no_self_parent"),
    )

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"))
    number: Mapped[int] = mapped_column(Integer)
    key: Mapped[str] = mapped_column(String(32))
    title: Mapped[str] = mapped_column(String(500))
    description: Mapped[str] = mapped_column(Text, default="")
    description_hash: Mapped[str] = mapped_column(String(64), default="")
    acceptance_criteria: Mapped[list[str]] = mapped_column(default=list)
    status: Mapped[str] = mapped_column(String(32), default="backlog")
    priority: Mapped[str] = mapped_column(String(16), default="medium")
    category: Mapped[str | None] = mapped_column(String(32), default=None, index=True)
    parent_id: Mapped[str | None] = mapped_column(
        ForeignKey("tasks.id", ondelete="RESTRICT"), index=True, default=None
    )
    assignee: Mapped[str | None] = mapped_column(String(200), default=None, index=True)
    assigned_agent: Mapped[str | None] = mapped_column(String(100), default=None)
    claimed_by: Mapped[str | None] = mapped_column(String(200), default=None)
    claim_expires_at: Mapped[datetime | None] = mapped_column(default=None)
    auto_execute: Mapped[bool] = mapped_column(Boolean, default=False)
    estimated_effort: Mapped[dict[str, Any] | None] = mapped_column(default=None)
    metadata_: Mapped[dict[str, Any]] = mapped_column("metadata", default=dict)
    due_at: Mapped[datetime | None] = mapped_column(default=None)
    scheduled_at: Mapped[datetime | None] = mapped_column(default=None)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    version: Mapped[int] = mapped_column(Integer, default=1)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()
    started_at: Mapped[datetime | None] = mapped_column(default=None)
    completed_at: Mapped[datetime | None] = mapped_column(default=None)
    accepted_at: Mapped[datetime | None] = mapped_column(default=None)
    archived_at: Mapped[datetime | None] = mapped_column(default=None)
    current_classification_id: Mapped[str | None] = mapped_column(String(36), default=None)

    tags: Mapped[list[TaskTag]] = relationship(cascade="all, delete-orphan", order_by="TaskTag.tag")


class TaskTag(Base):
    __tablename__ = "task_tags"

    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), primary_key=True)
    tag: Mapped[str] = mapped_column(String(100), primary_key=True, index=True)


class TaskProject(Base):
    __tablename__ = "task_projects"

    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), primary_key=True)
    project_id: Mapped[str] = mapped_column(
        ForeignKey("projects.id", ondelete="CASCADE"), primary_key=True, index=True
    )


class TaskRepository(Base):
    __tablename__ = "task_repositories"

    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), primary_key=True)
    repository_id: Mapped[str] = mapped_column(
        ForeignKey("repositories.id", ondelete="CASCADE"), primary_key=True, index=True
    )
    access: Mapped[str] = mapped_column(String(16), default="read_write")
    created_at: Mapped[datetime] = created_col()


class TaskDependency(Base):
    """``task_id`` cannot proceed until ``depends_on_id`` is accepted."""

    __tablename__ = "task_dependencies"
    __table_args__ = (
        UniqueConstraint("task_id", "depends_on_id"),
        CheckConstraint("task_id <> depends_on_id", name="no_self"),
    )

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    depends_on_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class TaskRelation(Base):
    __tablename__ = "task_relations"
    __table_args__ = (
        UniqueConstraint("source_task_id", "target_task_id", "relation_type"),
        CheckConstraint("source_task_id <> target_task_id", name="no_self"),
    )

    id: Mapped[str] = pk()
    source_task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    target_task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    relation_type: Mapped[str] = mapped_column(String(32))
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class TaskComment(Base):
    __tablename__ = "task_comments"

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    author: Mapped[str] = mapped_column(String(200))
    kind: Mapped[str] = mapped_column(String(16), default="comment")
    body: Mapped[str] = mapped_column(Text)
    data: Mapped[dict[str, Any]] = mapped_column(default=dict)
    created_at: Mapped[datetime] = created_col()


class Artifact(Base):
    __tablename__ = "artifacts"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    task_id: Mapped[str | None] = mapped_column(
        ForeignKey("tasks.id", ondelete="SET NULL"), index=True, default=None
    )
    attempt_id: Mapped[str | None] = mapped_column(String(36), index=True, default=None)
    kind: Mapped[str] = mapped_column(String(32))
    name: Mapped[str] = mapped_column(String(300))
    media_type: Mapped[str] = mapped_column(String(100), default="application/octet-stream")
    path: Mapped[str] = mapped_column(String(1000))
    size_bytes: Mapped[int] = mapped_column(Integer, default=0)
    sha256: Mapped[str] = mapped_column(String(64), default="")
    retention: Mapped[str] = mapped_column(String(16), default="keep")  # keep | telemetry
    purged_at: Mapped[datetime | None] = mapped_column(default=None)
    metadata_: Mapped[dict[str, Any]] = mapped_column("metadata", default=dict)
    created_at: Mapped[datetime] = created_col()


class Event(Base):
    """Append-only domain event. UPDATE and DELETE are rejected by SQLite triggers."""

    __tablename__ = "events"
    __table_args__ = (
        Index("ix_events_entity", "entity_type", "entity_id"),
        Index("ix_events_ws_seq", "workspace_id", "seq"),
    )

    seq: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    id: Mapped[str] = mapped_column(String(36), unique=True)
    workspace_id: Mapped[str | None] = mapped_column(String(36), default=None)
    event_type: Mapped[str] = mapped_column(String(100), index=True)
    entity_type: Mapped[str] = mapped_column(String(50))
    entity_id: Mapped[str] = mapped_column(String(36))
    actor: Mapped[str] = mapped_column(String(200))
    actor_kind: Mapped[str] = mapped_column(String(16))
    occurred_at: Mapped[datetime] = created_col()
    correlation_id: Mapped[str] = mapped_column(String(36), index=True)
    causation_id: Mapped[str | None] = mapped_column(String(36), default=None)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None, index=True)
    schema_version: Mapped[int] = mapped_column(Integer, default=1)
    data: Mapped[dict[str, Any]] = mapped_column(default=dict)


class AgentProfile(Base):
    """A known agent identity and the permissions granted to it."""

    __tablename__ = "agent_profiles"
    __table_args__ = (UniqueConstraint("workspace_id", "name"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(100))
    adapter: Mapped[str | None] = mapped_column(String(100), default=None)
    model: Mapped[str | None] = mapped_column(String(200), default=None)
    permissions: Mapped[list[str]] = mapped_column(default=list)
    enabled: Mapped[bool] = mapped_column(Boolean, default=True)
    last_seen_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()


class Notification(Base):
    __tablename__ = "notifications"

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="CASCADE"), index=True)
    level: Mapped[str] = mapped_column(String(16), default="info")
    title: Mapped[str] = mapped_column(String(300))
    body: Mapped[str] = mapped_column(Text, default="")
    source: Mapped[str] = mapped_column(String(200), default="")
    data: Mapped[dict[str, Any]] = mapped_column(default=dict)
    read_at: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()


class SystemCursor(Base):
    """Named high-water marks (e.g. the automation engine's position in the event log)."""

    __tablename__ = "system_cursors"

    name: Mapped[str] = mapped_column(String(100), primary_key=True)
    value: Mapped[int] = mapped_column(Integer, default=0)
    updated_at: Mapped[datetime] = updated_col()
