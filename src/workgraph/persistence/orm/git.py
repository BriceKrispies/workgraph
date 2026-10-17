"""Ingested Git metadata. Full diffs stay in Git; only stable metadata is stored."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import Boolean, Float, ForeignKey, Index, Integer, String, Text, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from workgraph.persistence.orm.base import Base, created_col, pk, updated_col


class Commit(Base):
    __tablename__ = "commits"
    __table_args__ = (
        UniqueConstraint("repository_id", "sha"),
        Index("ix_commits_repo_time", "repository_id", "committed_at"),
    )

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"))
    sha: Mapped[str] = mapped_column(String(64))
    tree_sha: Mapped[str | None] = mapped_column(String(64), default=None)
    parents: Mapped[list[str]] = mapped_column(default=list)
    author_name: Mapped[str] = mapped_column(String(300), default="")
    author_email: Mapped[str] = mapped_column(String(300), default="")
    authored_at: Mapped[datetime | None] = mapped_column(default=None)
    committer_name: Mapped[str] = mapped_column(String(300), default="")
    committer_email: Mapped[str] = mapped_column(String(300), default="")
    committed_at: Mapped[datetime | None] = mapped_column(default=None)
    subject: Mapped[str] = mapped_column(String(1000), default="")
    message: Mapped[str] = mapped_column(Text, default="")
    trailers: Mapped[list[dict[str, Any]]] = mapped_column(default=list)
    is_merge: Mapped[bool] = mapped_column(Boolean, default=False)
    patch_id: Mapped[str | None] = mapped_column(String(64), default=None, index=True)
    insertions: Mapped[int] = mapped_column(Integer, default=0)
    deletions: Mapped[int] = mapped_column(Integer, default=0)
    files_changed: Mapped[int] = mapped_column(Integer, default=0)
    reachable: Mapped[bool] = mapped_column(Boolean, default=True)
    unreachable_since: Mapped[datetime | None] = mapped_column(default=None)
    object_available: Mapped[bool] = mapped_column(Boolean, default=True)
    structure_analyzed: Mapped[bool] = mapped_column(Boolean, default=False)
    branches: Mapped[list[str]] = mapped_column(default=list)
    ingested_at: Mapped[datetime] = created_col()


class CommitFileChange(Base):
    __tablename__ = "commit_file_changes"

    id: Mapped[str] = pk()
    commit_id: Mapped[str] = mapped_column(ForeignKey("commits.id", ondelete="CASCADE"), index=True)
    path: Mapped[str] = mapped_column(String(1000), index=True)
    old_path: Mapped[str | None] = mapped_column(String(1000), default=None)
    change_type: Mapped[str] = mapped_column(String(2))  # A M D R C T
    similarity: Mapped[int | None] = mapped_column(Integer, default=None)
    insertions: Mapped[int] = mapped_column(Integer, default=0)
    deletions: Mapped[int] = mapped_column(Integer, default=0)
    is_binary: Mapped[bool] = mapped_column(Boolean, default=False)
    language: Mapped[str | None] = mapped_column(String(32), default=None)
    file_class: Mapped[str] = mapped_column(String(16), default="production")
    module: Mapped[str] = mapped_column(String(300), default="")


class CommitSymbol(Base):
    """A language-level structure (function, class, ...) touched by a file change."""

    __tablename__ = "commit_symbols"

    id: Mapped[str] = pk()
    file_change_id: Mapped[str] = mapped_column(
        ForeignKey("commit_file_changes.id", ondelete="CASCADE"), index=True
    )
    symbol_kind: Mapped[str] = mapped_column(String(32))
    name: Mapped[str] = mapped_column(String(500))
    change_kind: Mapped[str] = mapped_column(String(16))  # added | modified | deleted
    start_line: Mapped[int | None] = mapped_column(Integer, default=None)
    end_line: Mapped[int | None] = mapped_column(Integer, default=None)
    lines_changed: Mapped[int] = mapped_column(Integer, default=0)


class CommitAnalysis(Base):
    """Provenance record for the structural analysis of one commit."""

    __tablename__ = "commit_analyses"

    commit_id: Mapped[str] = mapped_column(ForeignKey("commits.id", ondelete="CASCADE"), primary_key=True)
    analyzer_version: Mapped[str] = mapped_column(String(32))
    files_parsed: Mapped[int] = mapped_column(Integer, default=0)
    files_fallback: Mapped[int] = mapped_column(Integer, default=0)
    functions_affected: Mapped[int] = mapped_column(Integer, default=0)
    types_affected: Mapped[int] = mapped_column(Integer, default=0)
    detail: Mapped[dict[str, Any]] = mapped_column(default=dict)
    analyzed_at: Mapped[datetime] = created_col()


class CommitEquivalence(Base):
    """Two commits carrying the same change (rebase/amend rewrite or cherry-pick)."""

    __tablename__ = "commit_equivalences"
    __table_args__ = (UniqueConstraint("original_commit_id", "equivalent_commit_id"),)

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"), index=True)
    original_commit_id: Mapped[str] = mapped_column(ForeignKey("commits.id", ondelete="CASCADE"))
    equivalent_commit_id: Mapped[str] = mapped_column(ForeignKey("commits.id", ondelete="CASCADE"))
    kind: Mapped[str] = mapped_column(String(16))  # rewrite | cherry_pick
    evidence: Mapped[dict[str, Any]] = mapped_column(default=dict)
    created_at: Mapped[datetime] = created_col()


class CommitTaskLink(Base):
    __tablename__ = "commit_task_links"
    __table_args__ = (UniqueConstraint("commit_id", "task_id", "source"),)

    id: Mapped[str] = pk()
    commit_id: Mapped[str] = mapped_column(ForeignKey("commits.id", ondelete="CASCADE"), index=True)
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    source: Mapped[str] = mapped_column(String(32))
    confidence: Mapped[float] = mapped_column(Float)
    evidence: Mapped[dict[str, Any]] = mapped_column(default=dict)
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None)
    created_by: Mapped[str] = mapped_column(String(200), default="system")
    created_at: Mapped[datetime] = created_col()


class RepositoryRef(Base):
    __tablename__ = "repository_refs"
    __table_args__ = (UniqueConstraint("repository_id", "name"),)

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"), index=True)
    name: Mapped[str] = mapped_column(String(500))
    sha: Mapped[str] = mapped_column(String(64))
    updated_at: Mapped[datetime] = updated_col()


class TaskChangeSurface(Base):
    """Final change surface: starting revision versus accepted revision, per repository."""

    __tablename__ = "task_change_surfaces"
    __table_args__ = (UniqueConstraint("task_id", "repository_id", "attempt_id"),)

    id: Mapped[str] = pk()
    task_id: Mapped[str] = mapped_column(ForeignKey("tasks.id", ondelete="CASCADE"), index=True)
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"))
    attempt_id: Mapped[str | None] = mapped_column(String(36), default=None)
    base_sha: Mapped[str] = mapped_column(String(64))
    final_sha: Mapped[str] = mapped_column(String(64))
    files_changed: Mapped[int] = mapped_column(Integer, default=0)
    insertions: Mapped[int] = mapped_column(Integer, default=0)
    deletions: Mapped[int] = mapped_column(Integer, default=0)
    renames: Mapped[int] = mapped_column(Integer, default=0)
    binary_files: Mapped[int] = mapped_column(Integer, default=0)
    functions_affected: Mapped[int] = mapped_column(Integer, default=0)
    types_affected: Mapped[int] = mapped_column(Integer, default=0)
    detail: Mapped[dict[str, Any]] = mapped_column(default=dict)
    computed_at: Mapped[datetime] = created_col()
    updated_at: Mapped[datetime] = updated_col()


class GitObjectGap(Base):
    """Records that a historical Git object referenced by Workgraph is no longer accessible."""

    __tablename__ = "git_object_gaps"
    __table_args__ = (UniqueConstraint("repository_id", "sha"),)

    id: Mapped[str] = pk()
    repository_id: Mapped[str] = mapped_column(ForeignKey("repositories.id", ondelete="CASCADE"))
    sha: Mapped[str] = mapped_column(String(64))
    context: Mapped[str] = mapped_column(Text, default="")
    detected_at: Mapped[datetime] = created_col()
