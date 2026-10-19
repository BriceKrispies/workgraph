"""Shared types between the tracker client, the issue mapper and the sync service."""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Protocol

SYNC_COMPONENT = "tracker-sync"
SYNC_ACTOR = f"system:{SYNC_COMPONENT}"
"""The only actor allowed to change tracker-owned fields of a tracked task."""

STATUS_CATEGORIES = ("new", "indeterminate", "done")


@dataclass(frozen=True)
class PullRequestRef:
    """A pull request named by an issue."""

    url: str
    normalized_url: str
    """``github.com/org/repo/pull/42``: lowercase host, no scheme, no trailing slash or fragment."""
    provider: str
    """``github`` for now."""
    repo_full_name: str
    """``org/repo``."""
    repo_normalized_url: str
    """``github.com/org/repo``, comparable with ``repository_remotes.normalized_url``."""
    number: int
    sources: tuple[str, ...]
    """Which extractors found it: ``custom_field`` and/or ``dev_panel``."""
    title: str = ""


@dataclass(frozen=True)
class TrackerSnapshot:
    """What Workgraph takes from one tracker issue. Produced by a pure mapper from the issue JSON."""

    external_id: str
    key: str
    url: str
    project_key: str
    title: str
    description: str
    priority: str
    """A Workgraph priority: critical | high | medium | low."""
    assignee: str | None
    status_name: str
    status_category: str
    """One of STATUS_CATEGORIES."""
    resolution: str | None
    parent_key: str | None
    blocked_by_keys: tuple[str, ...]
    """Keys of issues that block this one (Jira "is blocked by" links)."""
    pull_requests: tuple[PullRequestRef, ...]
    remote_updated_at: datetime
    issue_type: str = ""
    labels: tuple[str, ...] = ()
    raw: dict[str, Any] = field(default_factory=dict, compare=False)


class TrackerClient(Protocol):
    """Read-only access to a tracker. Implementations issue GET requests only."""

    def search_updated(self, projects: Sequence[str], since: datetime | None) -> Iterator[dict[str, Any]]:
        """Issues in ``projects`` updated at or after ``since`` (all when None), oldest update first.

        Each item is the issue JSON with at least ``id``, ``key`` and ``fields``.
        """
        ...

    def list_keys(self, projects: Sequence[str]) -> Iterator[tuple[str, str]]:
        """``(external_id, key)`` for every issue currently in ``projects``."""
        ...

    def get_issue(self, key: str) -> dict[str, Any] | None:
        """One issue's JSON, or None when it does not exist or is not visible."""
        ...

    def dev_pull_requests(self, external_id: str) -> list[dict[str, Any]]:
        """Pull requests from the development panel; raw dev-status ``pullRequests`` entries."""
        ...

    def close(self) -> None: ...


@dataclass
class SyncReport:
    connection: str
    full: bool
    fetched: int = 0
    created: int = 0
    updated: int = 0
    unchanged: int = 0
    missing: int = 0
    archived: int = 0
    unresolved_pull_requests: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    cursor: datetime | None = None

    def as_dict(self) -> dict[str, Any]:
        return {
            "connection": self.connection,
            "full": self.full,
            "fetched": self.fetched,
            "created": self.created,
            "updated": self.updated,
            "unchanged": self.unchanged,
            "missing": self.missing,
            "archived": self.archived,
            "unresolved_pull_requests": list(self.unresolved_pull_requests),
            "errors": list(self.errors),
            "cursor": self.cursor.isoformat() if self.cursor else None,
        }
