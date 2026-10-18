"""Stacks: an integration unit that groups a parent task's child changes."""

from __future__ import annotations

from datetime import datetime

from sqlalchemy import ForeignKey, Index, Integer, String, Text, UniqueConstraint, text
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class Stack(Base):
    __tablename__ = "stacks"
    __table_args__ = (
        # A parent has at most one stack that is still in progress; landed and abandoned stacks are history.
        Index(
            "ix_stacks_active_parent",
            "parent_task_id",
            unique=True,
            sqlite_where=text("status NOT IN ('landed', 'abandoned')"),
        ),
    )

    id: Mapped[str] = pk()
    workspace_id: Mapped[str] = mapped_column(ForeignKey("workspaces.id", ondelete="RESTRICT"), index=True)
    parent_task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="RESTRICT"), index=True)
    status: Mapped[str] = mapped_column(String(16), default="open")
    # JSON request to land the stack once it verifies on a moved default branch; null when none is pending.
    land_request: Mapped[str | None] = mapped_column(Text, default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class StackRepository(Base):
    """A repository taking part in a stack, with the commit its integration branch starts from."""

    __tablename__ = "stack_repositories"

    stack_id: Mapped[str] = mapped_column(ForeignKey("stacks.id", ondelete="CASCADE"), primary_key=True)
    repository_id: Mapped[str] = mapped_column(
        ForeignKey("repositories.id", ondelete="RESTRICT"), primary_key=True, index=True
    )
    base_sha: Mapped[str] = mapped_column(String(64))
    integration_branch: Mapped[str] = mapped_column(String(300))
    created_at: Mapped[datetime] = created_col()


class StackItem(Base):
    """One child task's place in a stack, tracked separately from the task lifecycle."""

    __tablename__ = "stack_items"
    __table_args__ = (UniqueConstraint("stack_id", "task_id"),)

    id: Mapped[str] = pk()
    stack_id: Mapped[str] = mapped_column(ForeignKey("stacks.id", ondelete="CASCADE"), index=True)
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="RESTRICT"), index=True)
    state: Mapped[str] = mapped_column(String(16), default="pending")
    merge_order: Mapped[int | None] = mapped_column(Integer, default=None)
    detail: Mapped[str | None] = mapped_column(Text, default=None)
    created_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()
