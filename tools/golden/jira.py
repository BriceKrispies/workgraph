"""Golden vectors for the Jira mapper and tracker status policy (GV-08).

Writes golden/jira/*.json from the Python implementation in
src/workgraph/integrations/trackers/mapper.py and src/workgraph/core/policies/trackers.py.
Run with: uv run python tools/golden/jira.py
"""

from __future__ import annotations

import itertools
import json
from pathlib import Path
from typing import Any

from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.models.enums import TaskStatus
from workgraph.core.policies.trackers import tracker_target_status
from workgraph.integrations.trackers.mapper import adf_to_text, map_issue, parse_pull_request_url
from workgraph.integrations.trackers.types import PullRequestRef, TrackerSnapshot

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURES = REPO_ROOT / "tests" / "fixtures" / "jira"
OUT_DIR = REPO_ROOT / "golden" / "jira"


def _load(name: str) -> dict[str, Any]:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def _config_values(**overrides: Any) -> dict[str, Any]:
    values: dict[str, Any] = {
        "base_url": "https://acme.atlassian.net",
        "workspace": "work",
        "projects": ["ABC"],
        "token_env": "WORKGRAPH_JIRA_TOKEN",
        "pr_fields": ["customfield_10042"],
    }
    values.update(overrides)
    return values


def _issue(**fields: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "summary": "Minimal issue",
        "project": {"key": "ABC"},
        "updated": "2026-10-18T09:15:02.123+0000",
    }
    base.update(fields)
    return {"id": "1", "key": "ABC-1", "fields": base}


def _pr_entry(url: str, name: str = "") -> dict[str, Any]:
    return {"url": url, "name": name, "status": "OPEN"}


def _ref_json(ref: PullRequestRef) -> dict[str, Any]:
    return {
        "url": ref.url,
        "normalized_url": ref.normalized_url,
        "provider": ref.provider,
        "repo_full_name": ref.repo_full_name,
        "repo_normalized_url": ref.repo_normalized_url,
        "number": ref.number,
        "sources": list(ref.sources),
        "title": ref.title,
    }


def _snapshot_json(snap: TrackerSnapshot) -> dict[str, Any]:
    return {
        "external_id": snap.external_id,
        "key": snap.key,
        "url": snap.url,
        "project_key": snap.project_key,
        "title": snap.title,
        "description": snap.description,
        "priority": snap.priority,
        "assignee": snap.assignee,
        "status_name": snap.status_name,
        "status_category": snap.status_category,
        "resolution": snap.resolution,
        "parent_key": snap.parent_key,
        "blocked_by_keys": list(snap.blocked_by_keys),
        "pull_requests": [_ref_json(ref) for ref in snap.pull_requests],
        "remote_updated_at": snap.remote_updated_at.isoformat(),
        "issue_type": snap.issue_type,
        "labels": list(snap.labels),
        "raw": snap.raw,
    }


def _map_case(name: str, issue: dict[str, Any], dev: list[dict[str, Any]], config: dict[str, Any]) -> dict:
    """One map_issue case: inputs as data, expected as the snapshot or the raised error."""
    case: dict[str, Any] = {"name": name, "issue": issue, "dev_pull_requests": dev, "config": config}
    try:
        snap = map_issue(issue, dev, JiraTrackerConfig(**config))
    except Exception as exc:  # the error type and message are part of the contract
        case["error"] = {"type": type(exc).__name__, "message": str(exc)}
    else:
        case["expected"] = _snapshot_json(snap)
    return case


def map_issue_cases() -> list[dict[str, Any]]:
    story = _load("cloud_story.json")
    story_dev = _load("dev_status.json")["detail"][0]["pullRequests"]
    done = _load("cloud_done_wont_do.json")
    dc = _load("datacenter_issue.json")
    cases: list[dict[str, Any]] = [
        _map_case("cloud-story-dev-panel", story, story_dev, _config_values(dev_panel=True)),
        _map_case("cloud-story-no-dev-panel", story, story_dev, _config_values(dev_panel=False)),
        _map_case("done-wont-do", done, [], _config_values()),
        _map_case(
            "datacenter-issue",
            dc,
            [],
            _config_values(
                base_url="https://jira.corp.example/",
                projects=["OPS"],
                pr_fields=["customfield_11000"],
                dev_panel=False,
            ),
        ),
    ]
    priority_names: list[str | None] = [
        "Highest",
        "Blocker",
        "Critical",
        "High",
        "Major",
        "Medium",
        "Low",
        "Lowest",
        "Minor",
        "Trivial",
        "HIGH",
        "  minor ",
        "Showstopper",
        None,
    ]
    for jira_name in priority_names:
        fields: dict[str, Any] = {"priority": None if jira_name is None else {"name": jira_name}}
        cases.append(_map_case(f"priority-{jira_name!r}", _issue(**fields), [], _config_values()))
    cases.append(_map_case("priority-field-missing", _issue(), [], _config_values()))
    for category in ["new", "indeterminate", "done", "DONE"]:
        fields = {"status": {"name": "Whatever", "statusCategory": {"key": category}}}
        cases.append(_map_case(f"status-category-{category}", _issue(**fields), [], _config_values()))
    unknown_statuses: list[dict[str, Any]] = [
        {"name": "Mystery", "statusCategory": {"key": "undefined"}},
        {"name": "Mystery", "statusCategory": {}},
        {"name": "Mystery"},
    ]
    for i, status in enumerate(unknown_statuses):
        cases.append(_map_case(f"unknown-status-{i}", _issue(status=status), [], _config_values()))
    cases.append(_map_case("missing-optional-fields", _issue(), [], _config_values()))
    cases.append(
        _map_case(
            "assignee-empty-display-name",
            _issue(assignee={"accountId": "x", "displayName": ""}),
            [],
            _config_values(),
        )
    )
    no_project = _issue()
    del no_project["fields"]["project"]
    cases.append(_map_case("project-key-from-issue-key", no_project, [], _config_values()))
    cases.append(
        _map_case(
            "base-url-trailing-slash",
            _issue(),
            [],
            _config_values(base_url="https://acme.atlassian.net/"),
        )
    )
    links = [
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-2"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "outwardIssue": {"key": "ABC-5"}},
        {"type": {"name": "Duplicate"}, "inwardIssue": {"key": "ABC-7"}},
    ]
    cases.append(_map_case("blocked-by-sorted-dedup", _issue(issuelinks=links), [], _config_values()))
    cases.append(_map_case("parent-null", _issue(parent=None), [], _config_values()))
    cases.append(
        _map_case(
            "timestamp-offset-converted",
            _issue(updated="2026-10-18T11:15:02.500+0200"),
            [],
            _config_values(),
        )
    )
    no_updated = _issue()
    del no_updated["fields"]["updated"]
    cases.append(_map_case("timestamp-missing-error", no_updated, [], _config_values()))
    cases.append(
        _map_case(
            "timestamp-z-suffix",
            _issue(updated="2026-10-18T09:15:02Z"),
            [],
            _config_values(),
        )
    )
    cases.append(
        _map_case(
            "timestamp-no-fraction-offset",
            _issue(updated="2026-10-18T09:15:02+0000"),
            [],
            _config_values(),
        )
    )

    none_shapes: list[Any] = [
        None,
        "",
        "no links here",
        {"type": "doc", "version": 1, "content": []},
        ["https://github.com/acme/svc-billing/issues/9"],
    ]
    for i, value in enumerate(none_shapes):
        cases.append(
            _map_case(f"custom-field-no-pr-{i}", _issue(customfield_10042=value), [], _config_values())
        )
    pr_shapes: list[Any] = [
        "https://github.com/acme/svc-api/pull/5",
        ["https://github.com/acme/svc-api/pull/5 and more text"],
        {"url": "https://github.com/acme/svc-api/pull/5", "value": "ignored"},
        {
            "type": "doc",
            "content": [{"type": "inlineCard", "attrs": {"url": "https://github.com/acme/svc-api/pull/5"}}],
        },
        {
            "type": "doc",
            "content": [
                {
                    "type": "paragraph",
                    "content": [
                        {
                            "type": "text",
                            "text": "svc-api",
                            "marks": [
                                {"type": "link", "attrs": {"href": "https://github.com/acme/svc-api/pull/5"}}
                            ],
                        }
                    ],
                }
            ],
        },
    ]
    for i, value in enumerate(pr_shapes):
        cases.append(
            _map_case(f"custom-field-shape-{i}", _issue(customfield_10042=value), [], _config_values())
        )
    cases.append(
        _map_case(
            "trailing-punctuation",
            _issue(customfield_10042="See https://github.com/acme/svc-api/pull/5."),
            [],
            _config_values(),
        )
    )
    cases.append(
        _map_case(
            "several-pr-fields",
            _issue(
                customfield_10042="https://github.com/acme/a/pull/1",
                customfield_10043=["https://github.com/acme/b/pull/2"],
            ),
            [],
            _config_values(pr_fields=["customfield_10042", "customfield_10043"]),
        )
    )
    cases.append(
        _map_case(
            "same-pr-twice-in-custom-field",
            _issue(
                customfield_10042=[
                    "https://github.com/acme/svc/pull/3",
                    "https://github.com/acme/svc/pull/3/files",
                ]
            ),
            [],
            _config_values(),
        )
    )
    cases.append(
        _map_case(
            "dev-panel-title-kept",
            _issue(customfield_10042="https://github.com/acme/svc/pull/3"),
            [_pr_entry("https://github.com/acme/svc/pull/3", name="Dev title")],
            _config_values(),
        )
    )
    cases.append(
        _map_case(
            "dev-panel-non-pr-skipped",
            _issue(),
            [{"url": "", "name": "x"}, {"url": "https://github.com/acme/svc/issues/3", "name": "y"}],
            _config_values(dev_panel=True),
        )
    )
    cases.append(
        _map_case(
            "pull-requests-sorted",
            _issue(),
            [
                _pr_entry("https://github.com/zeta/svc/pull/1"),
                _pr_entry("https://github.com/alpha/svc/pull/2"),
            ],
            _config_values(),
        )
    )
    return cases


ADF_CASES: list[tuple[str, Any]] = [
    (
        "common-nodes",
        {
            "type": "doc",
            "content": [
                {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Goal"}]},
                {
                    "type": "orderedList",
                    "content": [
                        {
                            "type": "listItem",
                            "content": [{"type": "paragraph", "content": [{"type": "text", "text": "one"}]}],
                        },
                        {
                            "type": "listItem",
                            "content": [{"type": "paragraph", "content": [{"type": "text", "text": "two"}]}],
                        },
                    ],
                },
                {"type": "codeBlock", "content": [{"type": "text", "text": "x = 1"}]},
                {
                    "type": "paragraph",
                    "content": [
                        {
                            "type": "text",
                            "text": "docs",
                            "marks": [{"type": "link", "attrs": {"href": "https://x.test/d"}}],
                        },
                        {"type": "emoji", "attrs": {"shortName": ":tada:", "text": "🎉"}},
                    ],
                },
                {"type": "paragraph", "content": []},
            ],
        },
    ),
    ("non-dict-none", None),
    ("non-dict-string", "plain"),
    (
        "cloud-story-description",
        _load("cloud_story.json")["fields"]["description"],
    ),
    (
        "hard-break-and-mention-and-card",
        {
            "type": "doc",
            "content": [
                {
                    "type": "paragraph",
                    "content": [
                        {"type": "text", "text": "Line one"},
                        {"type": "hardBreak"},
                        {"type": "text", "text": "Owner: "},
                        {"type": "mention", "attrs": {"id": "u1", "text": "@Priya Rao"}},
                        {"type": "status", "attrs": {"text": "DONE"}},
                    ],
                }
            ],
        },
    ),
    (
        "nested-bullet-list",
        {
            "type": "doc",
            "content": [
                {
                    "type": "bulletList",
                    "content": [
                        {
                            "type": "listItem",
                            "content": [
                                {"type": "paragraph", "content": [{"type": "text", "text": "outer"}]},
                                {
                                    "type": "bulletList",
                                    "content": [
                                        {
                                            "type": "listItem",
                                            "content": [
                                                {
                                                    "type": "paragraph",
                                                    "content": [{"type": "text", "text": "inner"}],
                                                }
                                            ],
                                        }
                                    ],
                                },
                            ],
                        }
                    ],
                }
            ],
        },
    ),
    ("empty-doc", {"type": "doc", "content": []}),
]


PR_URLS: list[str] = [
    "https://github.com/org/repo/pull/42",
    "https://github.com/org/repo/pull/42/files",
    "https://github.com/org/repo/pull/42/commits",
    "https://github.com/org/repo/pull/42/?tab=checks#discussion_r1",
    "http://www.GitHub.com/Org/Repo/pull/42",
    "https://github.example.com/Team/Svc/pull/7",
    "https://github.com/org/repo/issues/42",
    "https://github.com/org/repo",
    "https://github.com/org/repo/",
    "https://github.com/org/repo/pull/",
    "https://github.com/org/repo/pull/abc",
    "https://github.com/org/repo/pull/0",
    "https://github.com/org/repo/pull/42/reviews",
    "https://gist.github.com/user/abc123",
    "https://github.com/org/repo/pulls",
    "github.com/org/repo/pull/42",
    "ftp://github.com/org/repo/pull/42",
    "not a url",
    "",
]


def pr_url_cases() -> list[dict[str, Any]]:
    cases = []
    for url in PR_URLS:
        ref = parse_pull_request_url(url)
        cases.append({"url": url, "expected": None if ref is None else _ref_json(ref)})
    return cases


STATUS_CATEGORY_INPUTS = ["new", "indeterminate", "done", "DONE", "Done", "undefined", ""]
RESOLUTION_INPUTS: list[str | None] = [None, "Won't Do", "won't do", "Duplicate", "Done", "Fixed"]
CANCELLED_INPUTS: list[list[str]] = [["Won't Do", "Duplicate"], [], ["fixed"]]


def status_cases() -> list[dict[str, Any]]:
    cases = []
    for current, category, resolution, cancelled in itertools.product(
        list(TaskStatus), STATUS_CATEGORY_INPUTS, RESOLUTION_INPUTS, CANCELLED_INPUTS
    ):
        target = tracker_target_status(current, category, resolution, cancelled)
        cases.append(
            {
                "current": current.value,
                "category": category,
                "resolution": resolution,
                "cancelled_resolutions": cancelled,
                "expected": None if target is None else target.value,
            }
        )
    return cases


def _write(name: str, payload: Any) -> None:
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT_DIR / name).write_text(text, encoding="utf-8", newline="\n")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    _write("map_issue.json", {"cases": map_issue_cases()})
    _write(
        "adf_to_text.json",
        {"cases": [{"name": name, "input": node, "expected": adf_to_text(node)} for name, node in ADF_CASES]},
    )
    _write("pr_url.json", {"cases": pr_url_cases()})
    _write("status_mapping.json", {"cases": status_cases()})


if __name__ == "__main__":
    main()
