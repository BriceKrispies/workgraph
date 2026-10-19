"""An in-memory tracker for the sync tests: a read-only client plus a simplified issue mapper.

Issues are plain dicts: ``{"id", "key", "fields": {...}}`` where ``fields`` carries the simplified
values listed in ``ISSUE_DEFAULTS``. ``fake_mapper`` turns them into ``TrackerSnapshot``s the way the
real Jira mapper would, so the sync can be exercised without HTTP.
"""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from dataclasses import replace
from datetime import datetime
from typing import Any

from workgraph.config.settings import TrackerConfig
from workgraph.integrations.trackers.types import PullRequestRef, TrackerSnapshot

BASE_URL = "https://acme.atlassian.net"

ISSUE_DEFAULTS: dict[str, Any] = {
    "updated": "2026-10-01T10:00:00+00:00",
    "title": "An issue",
    "description": "",
    "priority": "medium",
    "assignee": None,
    "status_name": "To Do",
    "status_category": "new",
    "resolution": None,
    "parent_key": None,
    "blocked_by_keys": [],
    "pull_requests": [],
}


def issue(key: str, *, id: str | None = None, **fields: Any) -> dict[str, Any]:
    """One simplified issue; keyword arguments override ``ISSUE_DEFAULTS``."""
    return {"id": id or f"id-{key}", "key": key, "fields": {**ISSUE_DEFAULTS, **fields}}


def snapshot(**overrides: Any) -> TrackerSnapshot:
    """A ``TrackerSnapshot`` with sensible defaults; keyword arguments override them."""
    values: dict[str, Any] = {
        "external_id": "id-ABC-1",
        "key": "ABC-1",
        "url": f"{BASE_URL}/browse/ABC-1",
        "project_key": "ABC",
        "title": "An issue",
        "description": "",
        "priority": "medium",
        "assignee": None,
        "status_name": "To Do",
        "status_category": "new",
        "resolution": None,
        "parent_key": None,
        "blocked_by_keys": (),
        "pull_requests": (),
        "remote_updated_at": datetime.fromisoformat(ISSUE_DEFAULTS["updated"]),
        "raw": {},
    }
    values.update(overrides)
    return TrackerSnapshot(**values)


def pull_request_ref(url: str, sources: Sequence[str] = ("custom_field",), title: str = "") -> PullRequestRef:
    """Parse ``https://github.com/org/repo/pull/42`` into a ``PullRequestRef``."""
    _scheme, _, rest = url.partition("://")
    host, org, repo, _pull, number = rest.rstrip("/").split("/")
    host = host.lower()
    repo_full = f"{org}/{repo}"
    return PullRequestRef(
        url=url,
        normalized_url=f"{host}/{repo_full}/pull/{number}".lower(),
        provider="github",
        repo_full_name=repo_full,
        repo_normalized_url=f"{host}/{repo_full}".lower(),
        number=int(number),
        sources=tuple(sources),
        title=title,
    )


def fake_mapper(
    issue_json: dict[str, Any], dev_pull_requests: list[dict[str, Any]], config: TrackerConfig
) -> TrackerSnapshot:
    """Build a snapshot from a simplified issue; dev panel entries carry their PR url under ``url``."""
    fields = issue_json["fields"]
    key = issue_json["key"]
    refs: dict[str, PullRequestRef] = {}
    for url in fields["pull_requests"]:
        ref = pull_request_ref(url, ("custom_field",))
        refs[ref.normalized_url] = ref
    for entry in dev_pull_requests:
        ref = pull_request_ref(entry["url"], ("dev_panel",), entry.get("title", ""))
        if ref.normalized_url in refs:
            ref = replace(refs[ref.normalized_url], sources=("custom_field", "dev_panel"))
        refs[ref.normalized_url] = ref
    return TrackerSnapshot(
        external_id=issue_json["id"],
        key=key,
        url=f"{BASE_URL}/browse/{key}",
        project_key=key.rsplit("-", 1)[0],
        title=fields["title"],
        description=fields["description"],
        priority=fields["priority"],
        assignee=fields["assignee"],
        status_name=fields["status_name"],
        status_category=fields["status_category"],
        resolution=fields["resolution"],
        parent_key=fields["parent_key"],
        blocked_by_keys=tuple(fields["blocked_by_keys"]),
        pull_requests=tuple(refs.values()),
        remote_updated_at=datetime.fromisoformat(fields["updated"]),
        raw=issue_json,
    )


class FakeTrackerClient:
    """Implements ``TrackerClient`` over a dict of issues and records every call.

    ``calls`` holds ``(method, args)`` tuples. ``on_call`` (optional) runs before each call, which
    tests use to observe state at call time. ``closed`` tells whether ``close`` ran.
    """

    def __init__(
        self,
        issues: Sequence[dict[str, Any]] = (),
        *,
        fail_dev_panel: bool = False,
        fail_search: Exception | None = None,
        dev_panels: dict[str, list[dict[str, Any]]] | None = None,
    ) -> None:
        self.issues: dict[str, dict[str, Any]] = {item["id"]: item for item in issues}
        self.fail_dev_panel = fail_dev_panel
        self.fail_search = fail_search
        self.dev_panels = dev_panels or {}
        self.calls: list[tuple[str, tuple[Any, ...]]] = []
        self.on_call: Any = None
        self.closed = False

    def _record(self, method: str, *args: Any) -> None:
        self.calls.append((method, args))
        if self.on_call is not None:
            self.on_call(method, args)

    def method_calls(self, method: str) -> list[tuple[Any, ...]]:
        return [args for name, args in self.calls if name == method]

    def search_updated(self, projects: Sequence[str], since: datetime | None) -> Iterator[dict[str, Any]]:
        self._record("search_updated", tuple(projects), since)
        if self.fail_search is not None:
            raise self.fail_search
        matching = [item for item in self.issues.values() if self._in_projects(item, projects)]
        if since is not None:
            matching = [item for item in matching if _updated(item) >= since]
        yield from sorted(matching, key=_updated)

    def list_keys(self, projects: Sequence[str]) -> Iterator[tuple[str, str]]:
        self._record("list_keys", tuple(projects))
        for item in self.issues.values():
            if self._in_projects(item, projects):
                yield item["id"], item["key"]

    def get_issue(self, key: str) -> dict[str, Any] | None:
        self._record("get_issue", key)
        for item in self.issues.values():
            if item["key"] == key:
                return item
        return None

    def dev_pull_requests(self, external_id: str) -> list[dict[str, Any]]:
        self._record("dev_pull_requests", external_id)
        if self.fail_dev_panel:
            raise RuntimeError("dev-status endpoint returned 500")
        return list(self.dev_panels.get(external_id, []))

    def close(self) -> None:
        self._record("close")
        self.closed = True

    @staticmethod
    def _in_projects(item: dict[str, Any], projects: Sequence[str]) -> bool:
        return item["key"].rsplit("-", 1)[0] in projects


def _updated(item: dict[str, Any]) -> datetime:
    return datetime.fromisoformat(item["fields"]["updated"])
