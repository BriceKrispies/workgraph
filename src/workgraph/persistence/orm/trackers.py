"""Issues mirrored from an external tracker (Jira) and typed links such as pull requests."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import ForeignKey, Integer, String, Text, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class ExternalItem(Base):
    """The tracker issue a task mirrors. The tracker owns these fields; only the sync writes them."""

    __tablename__ = "external_items"
    __table_args__ = (UniqueConstraint("connection", "external_id"),)

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), unique=True)
    # Name of the `[trackers.<name>]` connection in configuration.
    connection: Mapped[str] = mapped_column(String(100))
    # The tracker's own stable id (Jira's numeric issue id), unchanged when the key changes.
    external_id: Mapped[str] = mapped_column(String(200))
    external_key: Mapped[str] = mapped_column(String(64), index=True)
    url: Mapped[str] = mapped_column(String(1000), default="")
    tracker_status: Mapped[str] = mapped_column(String(200), default="")
    # Jira statusCategory key: new | indeterminate | done.
    tracker_status_category: Mapped[str] = mapped_column(String(32), default="new")
    tracker_resolution: Mapped[str | None] = mapped_column(String(200), default=None)
    remote_updated_at: Mapped[datetime | None] = mapped_column(default=None)
    snapshot: Mapped[dict[str, Any]] = mapped_column(default=dict)
    snapshot_hash: Mapped[str] = mapped_column(String(64), default="")
    synced_at: Mapped[datetime | None] = mapped_column(default=None)
    # Set when a full sync no longer finds the issue; cleared when it reappears.
    missing_since: Mapped[datetime | None] = mapped_column(default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class TaskLink(Base):
    """A typed link from a task to something outside Workgraph (``kind = pull_request`` first)."""

    __tablename__ = "task_links"
    __table_args__ = (UniqueConstraint("task_id", "kind", "normalized_url"),)

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    kind: Mapped[str] = mapped_column(String(32))
    provider: Mapped[str] = mapped_column(String(100), default="")
    url: Mapped[str] = mapped_column(String(1000))
    normalized_url: Mapped[str] = mapped_column(String(1000), index=True)
    # e.g. "org/repo" for a GitHub pull request; empty for other kinds.
    repo_full_name: Mapped[str] = mapped_column(String(300), default="")
    number: Mapped[int | None] = mapped_column(Integer, default=None)
    # The registered repository this link resolved to; null while that repository is not registered.
    repository_id: Mapped[str | None] = mapped_column(
        ForeignKey("repositories.id", ondelete="SET NULL"), index=True, default=None
    )
    # Which extractors found the link, e.g. ["custom_field", "dev_panel"], or ["manual"].
    sources: Mapped[list[str]] = mapped_column(default=list)
    title: Mapped[str] = mapped_column(Text, default="")
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()
