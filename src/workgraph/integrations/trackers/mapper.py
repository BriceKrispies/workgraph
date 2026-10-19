"""Map Jira issue JSON to a TrackerSnapshot. Pure: no I/O. See docs/design/tracker-sync.md."""

from __future__ import annotations

import re
from collections.abc import Iterator
from dataclasses import replace
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlsplit

from workgraph.config.settings import TrackerConfig
from workgraph.integrations.git.cli import normalize_remote_url
from workgraph.integrations.trackers.types import (
    STATUS_CATEGORIES,
    PullRequestRef,
    TrackerSnapshot,
)

SOURCE_CUSTOM_FIELD = "custom_field"
SOURCE_DEV_PANEL = "dev_panel"

_PRIORITIES = {
    "highest": "critical",
    "blocker": "critical",
    "critical": "critical",
    "high": "high",
    "major": "high",
    "medium": "medium",
    "low": "low",
    "lowest": "low",
    "minor": "low",
    "trivial": "low",
}

_BLOCK_TYPES = frozenset(
    {
        "paragraph",
        "heading",
        "bulletList",
        "orderedList",
        "listItem",
        "codeBlock",
        "blockquote",
        "panel",
        "table",
        "tableRow",
        "tableHeader",
        "tableCell",
        "mediaSingle",
        "mediaGroup",
        "rule",
        "expand",
        "nestedExpand",
        "decisionList",
        "taskList",
    }
)

_URL_IN_TEXT = re.compile(r"https?://[^\s<>\"'\[\]()]+")
_TRAILING_PUNCTUATION = ".,;:!?"
_PULL_SUFFIXES = frozenset({"files", "commits", "checks"})
_JIRA_TIMESTAMP_FORMATS = ("%Y-%m-%dT%H:%M:%S.%f%z", "%Y-%m-%dT%H:%M:%S%z")


def map_issue(
    issue: dict[str, Any], dev_pull_requests: list[dict[str, Any]], config: TrackerConfig
) -> TrackerSnapshot:
    """Build the snapshot for one issue from its JSON and its development-panel pull requests."""
    fields = issue.get("fields") or {}
    key = str(issue["key"])
    project_key = _dig(fields, "project", "key") or key.rsplit("-", 1)[0]
    return TrackerSnapshot(
        external_id=str(issue["id"]),
        key=key,
        url=f"{config.base_url.rstrip('/')}/browse/{key}",
        project_key=str(project_key),
        title=str(fields.get("summary") or ""),
        description=_description_text(fields.get("description")),
        priority=_priority(_dig(fields, "priority", "name")),
        assignee=_dig(fields, "assignee", "displayName") or None,
        status_name=str(_dig(fields, "status", "name") or ""),
        status_category=_status_category(_dig(fields, "status", "statusCategory", "key")),
        resolution=_dig(fields, "resolution", "name") or None,
        parent_key=_dig(fields, "parent", "key") or None,
        blocked_by_keys=_blocked_by_keys(fields.get("issuelinks")),
        pull_requests=_pull_requests(fields, dev_pull_requests, config),
        remote_updated_at=_parse_timestamp(fields.get("updated")),
        issue_type=str(_dig(fields, "issuetype", "name") or ""),
        labels=tuple(str(label) for label in fields.get("labels") or []),
        raw=issue,
    )


def adf_to_text(node: Any) -> str:
    """Flatten an Atlassian Document Format tree to plain text.

    Blocks are separated by blank lines, list items start with ``- ``, hard breaks become newlines,
    mentions show their text and cards show their URL.
    """
    if not isinstance(node, dict):
        return ""
    return "\n\n".join(_blocks(node)).strip()


def parse_pull_request_url(url: str) -> PullRequestRef | None:
    """Parse ``https://<host>/<org>/<repo>/pull/<n>`` on any host, or None for anything else.

    Accepts a trailing ``/files``, ``/commits`` or ``/checks``, a query, a fragment and ``www.``.
    The normalized forms are lowercase; ``repo_full_name`` keeps the case it was given in.
    """
    try:
        parts = urlsplit(url.strip())
        host = parts.hostname
    except ValueError:
        return None
    if parts.scheme not in ("http", "https") or not host:
        return None
    segments = [segment for segment in parts.path.split("/") if segment]
    if len(segments) == 5 and segments[4] in _PULL_SUFFIXES:
        segments = segments[:4]
    if len(segments) != 4 or segments[2] != "pull" or not segments[3].isdigit():
        return None
    number = int(segments[3])
    if number < 1:
        return None
    if host.startswith("www."):
        host = host[len("www.") :]
    org, repo = segments[0], segments[1]
    repo_full_name = f"{org}/{repo}"
    return PullRequestRef(
        url=f"https://{host}/{repo_full_name}/pull/{number}",
        normalized_url=f"{host}/{repo_full_name}/pull/{number}".lower(),
        provider="github",
        repo_full_name=repo_full_name,
        repo_normalized_url=normalize_remote_url(f"https://{host}/{repo_full_name}").lower(),
        number=number,
        sources=(),
    )


# --- fields ---------------------------------------------------------------------------------------


def _dig(value: Any, *keys: str) -> Any:
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def _priority(name: Any) -> str:
    if not isinstance(name, str):
        return "medium"
    return _PRIORITIES.get(name.strip().lower(), "medium")


def _status_category(key: Any) -> str:
    if isinstance(key, str) and key.lower() in STATUS_CATEGORIES:
        return key.lower()
    return "new"


def _blocked_by_keys(links: Any) -> tuple[str, ...]:
    keys: set[str] = set()
    for link in links or []:
        if _dig(link, "type", "name") != "Blocks":
            continue
        inward = link.get("inwardIssue")
        if isinstance(inward, dict) and inward.get("key"):
            keys.add(str(inward["key"]))
    return tuple(sorted(keys))


def _parse_timestamp(value: Any) -> datetime:
    if not isinstance(value, str):
        raise ValueError("Jira issue has no fields.updated timestamp")
    for pattern in _JIRA_TIMESTAMP_FORMATS:
        try:
            return datetime.strptime(value, pattern).astimezone(UTC)
        except ValueError:
            continue
    return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(UTC)


def _description_text(value: Any) -> str:
    if isinstance(value, dict):
        return adf_to_text(value)
    if isinstance(value, str):
        return value
    return ""


# --- pull requests --------------------------------------------------------------------------------


def _pull_requests(
    fields: dict[str, Any], dev_pull_requests: list[dict[str, Any]], config: TrackerConfig
) -> tuple[PullRequestRef, ...]:
    found: list[PullRequestRef] = []
    for field_id in config.pr_fields:
        for text in _strings(fields.get(field_id)):
            for match in _URL_IN_TEXT.findall(text):
                ref = parse_pull_request_url(match.rstrip(_TRAILING_PUNCTUATION))
                if ref is not None:
                    found.append(replace(ref, sources=(SOURCE_CUSTOM_FIELD,)))
    if config.dev_panel:
        for entry in dev_pull_requests:
            ref = parse_pull_request_url(str(entry.get("url") or ""))
            if ref is not None:
                title = str(entry.get("name") or "")
                found.append(replace(ref, sources=(SOURCE_DEV_PANEL,), title=title))
    return _merge(found)


def _merge(found: list[PullRequestRef]) -> tuple[PullRequestRef, ...]:
    merged: dict[str, PullRequestRef] = {}
    for ref in found:
        current = merged.get(ref.normalized_url)
        if current is None:
            merged[ref.normalized_url] = ref
            continue
        merged[ref.normalized_url] = replace(
            current,
            sources=tuple(sorted({*current.sources, *ref.sources})),
            title=current.title or ref.title,
        )
    return tuple(sorted(merged.values(), key=lambda ref: ref.normalized_url))


def _strings(value: Any) -> Iterator[str]:
    """Every string inside a field value: plain strings, lists, ADF trees and ``{"url": ...}`` dicts."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from _strings(item)
    elif isinstance(value, list | tuple):
        for item in value:
            yield from _strings(item)


# --- ADF flattening -------------------------------------------------------------------------------


def _blocks(node: dict[str, Any]) -> list[str]:
    kind = node.get("type")
    children = [child for child in node.get("content") or [] if isinstance(child, dict)]
    if kind in ("bulletList", "orderedList"):
        items = [item for item in (_list_item(child) for child in children) if item]
        return ["\n".join(items)] if items else []
    if any(child.get("type") in _BLOCK_TYPES for child in children):
        return [text for child in children for text in _blocks(child)]
    text = _inline(node).strip()
    return [text] if text else []


def _list_item(node: dict[str, Any]) -> str:
    texts = _blocks(node)
    if not texts:
        return ""
    return "- " + "\n".join(texts)


def _inline(node: dict[str, Any]) -> str:
    kind = node.get("type")
    attrs = node.get("attrs") or {}
    if kind == "text":
        text = str(node.get("text") or "")
        href = _link_href(node)
        return text or href
    if kind == "hardBreak":
        return "\n"
    if kind in ("mention", "emoji", "status"):
        return str(attrs.get("text") or "")
    if kind in ("inlineCard", "blockCard", "embedCard"):
        return str(attrs.get("url") or "")
    return "".join(_inline(child) for child in node.get("content") or [] if isinstance(child, dict))


def _link_href(node: dict[str, Any]) -> str:
    for mark in node.get("marks") or []:
        if isinstance(mark, dict) and mark.get("type") == "link":
            return str(_dig(mark, "attrs", "href") or "")
    return ""
