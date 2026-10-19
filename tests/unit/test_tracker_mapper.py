"""Jira issue to TrackerSnapshot mapping, and pull request URL parsing. Pure: no database, no network."""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

from workgraph.config.settings import JiraTrackerConfig
from workgraph.integrations.git.cli import normalize_remote_url
from workgraph.integrations.trackers.mapper import adf_to_text, map_issue, parse_pull_request_url

FIXTURES = Path(__file__).resolve().parents[1] / "fixtures" / "jira"


def _load(name: str) -> dict[str, Any]:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def _config(**overrides: Any) -> JiraTrackerConfig:
    values: dict[str, Any] = {
        "base_url": "https://acme.atlassian.net",
        "workspace": "work",
        "projects": ["ABC"],
        "token_env": "WORKGRAPH_JIRA_TOKEN",
        "pr_fields": ["customfield_10042"],
    }
    values.update(overrides)
    return JiraTrackerConfig(**values)


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


# --- cloud story: the full mapping -------------------------------------------------------------


@pytest.fixture
def story() -> Any:
    dev = _load("dev_status.json")["detail"][0]["pullRequests"]
    return map_issue(_load("cloud_story.json"), dev, _config(dev_panel=True))


def test_scalar_fields_come_from_the_issue(story: Any) -> None:
    assert story.external_id == "10001"
    assert story.key == "ABC-123"
    assert story.url == "https://acme.atlassian.net/browse/ABC-123"
    assert story.project_key == "ABC"
    assert story.title == "Add retry budget to the billing client"
    assert story.priority == "high"
    assert story.assignee == "Brice Lehew"
    assert story.status_name == "In Progress"
    assert story.status_category == "indeterminate"
    assert story.resolution is None
    assert story.parent_key == "ABC-100"
    assert story.issue_type == "Story"
    assert story.labels == ("billing", "retries")
    assert story.raw["key"] == "ABC-123"


def test_remote_updated_at_is_aware_utc(story: Any) -> None:
    assert story.remote_updated_at == datetime(2026, 10, 18, 9, 15, 2, 123000, tzinfo=UTC)
    assert story.remote_updated_at.utcoffset() == UTC.utcoffset(None)


def test_adf_description_is_flattened(story: Any) -> None:
    assert story.description == (
        "Calls to the billing API fail intermittently. Bounded retries are the fix.\n\n"
        "- Cap attempts at 3\n- Back off exponentially\n\n"
        "Line one\nOwner: @Priya Rao see https://acme.atlassian.net/wiki/spaces/ENG/pages/1"
    )


def test_blocked_by_keys_come_from_inward_blocks_links_only(story: Any) -> None:
    # ABC-122 and ABC-118 (listed twice) are "is blocked by"; ABC-124 is an outward "blocks";
    # ABC-130 is a "Relates" link.
    assert story.blocked_by_keys == ("ABC-118", "ABC-122")


def test_pull_requests_merge_custom_field_and_dev_panel(story: Any) -> None:
    by_url = {ref.normalized_url: ref for ref in story.pull_requests}
    assert list(by_url) == [
        "github.com/acme/svc-api/pull/43",
        "github.com/acme/svc-billing/pull/42",
        "github.com/acme/svc-web/pull/7",
    ]

    duplicated = by_url["github.com/acme/svc-billing/pull/42"]
    assert duplicated.sources == ("custom_field", "dev_panel")
    assert duplicated.title == "ABC-123 Add retry budget"
    assert duplicated.url == "https://github.com/acme/svc-billing/pull/42"
    assert duplicated.repo_full_name == "acme/svc-billing"
    assert duplicated.repo_normalized_url == "github.com/acme/svc-billing"
    assert duplicated.number == 42
    assert duplicated.provider == "github"

    dev_only = by_url["github.com/acme/svc-api/pull/43"]
    assert dev_only.sources == ("dev_panel",)
    assert dev_only.title == "ABC-123 Retry budget config"

    custom_only = by_url["github.com/acme/svc-web/pull/7"]
    assert custom_only.sources == ("custom_field",)
    assert custom_only.title == ""
    assert custom_only.repo_full_name == "Acme/Svc-Web"
    assert custom_only.repo_normalized_url == "github.com/acme/svc-web"


def test_dev_panel_is_ignored_when_disabled() -> None:
    dev = _load("dev_status.json")["detail"][0]["pullRequests"]
    mapped = map_issue(_load("cloud_story.json"), dev, _config(dev_panel=False))
    assert [ref.normalized_url for ref in mapped.pull_requests] == [
        "github.com/acme/svc-billing/pull/42",
        "github.com/acme/svc-web/pull/7",
    ]
    assert mapped.pull_requests[0].sources == ("custom_field",)


# --- other issues -------------------------------------------------------------------------------


def test_done_issue_with_wont_do_resolution() -> None:
    mapped = map_issue(_load("cloud_done_wont_do.json"), [], _config())
    assert mapped.status_category == "done"
    assert mapped.resolution == "Won't Do"
    assert mapped.priority == "low"
    assert mapped.description == ""
    assert mapped.assignee is None
    assert mapped.parent_key is None
    assert mapped.blocked_by_keys == ()
    assert mapped.pull_requests == ()
    assert mapped.remote_updated_at == datetime(2026, 10, 17, 16, 40, tzinfo=UTC)


def test_data_center_issue_passes_wiki_markup_and_string_pr_list_through() -> None:
    config = _config(
        base_url="https://jira.corp.example/",
        projects=["OPS"],
        pr_fields=["customfield_11000"],
        dev_panel=False,
    )
    mapped = map_issue(_load("datacenter_issue.json"), [], config)
    assert mapped.url == "https://jira.corp.example/browse/OPS-7"
    assert mapped.project_key == "OPS"
    assert (
        mapped.description
        == "h2. Goal\n\nRotate the *signing key* without downtime.\n\n* step one\n* step two"
    )
    assert mapped.priority == "high"
    assert mapped.status_category == "new"
    assert mapped.blocked_by_keys == ("OPS-3",)
    assert [ref.normalized_url for ref in mapped.pull_requests] == [
        "github.com/acme/svc-auth/pull/10",
        "github.com/acme/svc-auth/pull/9",
    ]
    assert all(ref.sources == ("custom_field",) for ref in mapped.pull_requests)


# --- field rules ----------------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("jira_name", "expected"),
    [
        ("Highest", "critical"),
        ("Blocker", "critical"),
        ("Critical", "critical"),
        ("High", "high"),
        ("Major", "high"),
        ("Medium", "medium"),
        ("Low", "low"),
        ("Lowest", "low"),
        ("Minor", "low"),
        ("Trivial", "low"),
        ("HIGH", "high"),
        ("  minor ", "low"),
        ("Showstopper", "medium"),
        (None, "medium"),
    ],
)
def test_priority_mapping(jira_name: str | None, expected: str) -> None:
    fields = {"priority": None if jira_name is None else {"name": jira_name}}
    assert map_issue(_issue(**fields), [], _config()).priority == expected


def test_missing_priority_field_is_medium() -> None:
    assert map_issue(_issue(), [], _config()).priority == "medium"


@pytest.mark.parametrize(
    ("category", "expected"),
    [("new", "new"), ("indeterminate", "indeterminate"), ("done", "done"), ("DONE", "done")],
)
def test_status_category_keys(category: str, expected: str) -> None:
    fields = {"status": {"name": "Whatever", "statusCategory": {"key": category}}}
    assert map_issue(_issue(**fields), [], _config()).status_category == expected


@pytest.mark.parametrize(
    "status",
    [
        {"name": "Mystery", "statusCategory": {"key": "undefined"}},
        {"name": "Mystery", "statusCategory": {}},
        {"name": "Mystery"},
    ],
)
def test_unknown_status_category_is_new(status: dict[str, Any]) -> None:
    mapped = map_issue(_issue(status=status), [], _config())
    assert mapped.status_category == "new"
    assert mapped.status_name == "Mystery"


def test_missing_optional_fields_are_empty() -> None:
    mapped = map_issue(_issue(), [], _config())
    assert mapped.title == "Minimal issue"
    assert mapped.description == ""
    assert mapped.assignee is None
    assert mapped.resolution is None
    assert mapped.parent_key is None
    assert mapped.blocked_by_keys == ()
    assert mapped.issue_type == ""
    assert mapped.labels == ()
    assert mapped.status_name == ""


def test_assignee_without_display_name_is_none() -> None:
    assert map_issue(_issue(assignee={"accountId": "x", "displayName": ""}), [], _config()).assignee is None


def test_project_key_falls_back_to_key_prefix() -> None:
    issue = _issue()
    del issue["fields"]["project"]
    assert map_issue(issue, [], _config()).project_key == "ABC"


def test_base_url_trailing_slash_is_not_doubled() -> None:
    mapped = map_issue(_issue(), [], _config(base_url="https://acme.atlassian.net/"))
    assert mapped.url == "https://acme.atlassian.net/browse/ABC-1"


def test_blocked_by_is_sorted_and_deduplicated() -> None:
    links = [
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-2"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "outwardIssue": {"key": "ABC-5"}},
        {"type": {"name": "Duplicate"}, "inwardIssue": {"key": "ABC-7"}},
    ]
    assert map_issue(_issue(issuelinks=links), [], _config()).blocked_by_keys == ("ABC-2", "ABC-9")


def test_parent_key_is_none_without_parent() -> None:
    assert map_issue(_issue(parent=None), [], _config()).parent_key is None


def test_remote_timestamp_with_offset_is_converted_to_utc() -> None:
    mapped = map_issue(_issue(updated="2026-10-18T11:15:02.500+0200"), [], _config())
    assert mapped.remote_updated_at == datetime(2026, 10, 18, 9, 15, 2, 500000, tzinfo=UTC)


def test_missing_timestamp_is_an_error() -> None:
    issue = _issue()
    del issue["fields"]["updated"]
    with pytest.raises(ValueError, match="updated"):
        map_issue(issue, [], _config())


# --- pull requests from custom fields -------------------------------------------------------------


@pytest.mark.parametrize(
    "value",
    [
        None,
        "",
        "no links here",
        {"type": "doc", "version": 1, "content": []},
        ["https://github.com/acme/svc-billing/issues/9"],
    ],
)
def test_custom_field_without_pull_requests_gives_none(value: Any) -> None:
    mapped = map_issue(_issue(customfield_10042=value), [], _config())
    assert mapped.pull_requests == ()


@pytest.mark.parametrize(
    "value",
    [
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
    ],
)
def test_custom_field_shapes_all_yield_the_pull_request(value: Any) -> None:
    mapped = map_issue(_issue(customfield_10042=value), [], _config())
    assert [ref.normalized_url for ref in mapped.pull_requests] == ["github.com/acme/svc-api/pull/5"]
    assert mapped.pull_requests[0].sources == ("custom_field",)


def test_trailing_punctuation_is_not_part_of_the_url() -> None:
    mapped = map_issue(_issue(customfield_10042="See https://github.com/acme/svc-api/pull/5."), [], _config())
    assert mapped.pull_requests[0].url == "https://github.com/acme/svc-api/pull/5"


def test_several_pr_fields_are_all_read() -> None:
    config = _config(pr_fields=["customfield_10042", "customfield_10043"])
    issue = _issue(
        customfield_10042="https://github.com/acme/a/pull/1",
        customfield_10043=["https://github.com/acme/b/pull/2"],
    )
    mapped = map_issue(issue, [], config)
    assert [ref.normalized_url for ref in mapped.pull_requests] == [
        "github.com/acme/a/pull/1",
        "github.com/acme/b/pull/2",
    ]


def test_same_pull_request_named_twice_in_custom_fields_is_one_link() -> None:
    value = ["https://github.com/acme/svc/pull/3", "https://github.com/acme/svc/pull/3/files"]
    mapped = map_issue(_issue(customfield_10042=value), [], _config())
    assert len(mapped.pull_requests) == 1
    assert mapped.pull_requests[0].sources == ("custom_field",)


def test_dev_panel_title_is_kept_when_custom_field_has_none() -> None:
    dev = [_pr_entry("https://github.com/acme/svc/pull/3", name="Dev title")]
    mapped = map_issue(_issue(customfield_10042="https://github.com/acme/svc/pull/3"), dev, _config())
    assert mapped.pull_requests[0].title == "Dev title"
    assert mapped.pull_requests[0].sources == ("custom_field", "dev_panel")


def test_dev_panel_entries_without_a_pull_request_url_are_skipped() -> None:
    dev = [{"url": "", "name": "x"}, {"url": "https://github.com/acme/svc/issues/3", "name": "y"}]
    assert map_issue(_issue(), dev, _config(dev_panel=True)).pull_requests == ()


def test_pull_requests_are_sorted_by_normalized_url() -> None:
    dev = [
        _pr_entry("https://github.com/zeta/svc/pull/1"),
        _pr_entry("https://github.com/alpha/svc/pull/2"),
    ]
    mapped = map_issue(_issue(), dev, _config())
    assert [ref.normalized_url for ref in mapped.pull_requests] == [
        "github.com/alpha/svc/pull/2",
        "github.com/zeta/svc/pull/1",
    ]


# --- ADF flattening -------------------------------------------------------------------------------


def test_adf_flattening_covers_the_common_nodes() -> None:
    doc = {
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
    }
    assert adf_to_text(doc) == "Goal\n\n- one\n- two\n\nx = 1\n\ndocs🎉"


def test_adf_to_text_of_non_dict_is_empty() -> None:
    assert adf_to_text(None) == ""
    assert adf_to_text("plain") == ""


# --- pull request URL parsing ---------------------------------------------------------------------


@pytest.mark.parametrize(
    ("url", "normalized_url", "repo_full_name", "repo_normalized_url", "number"),
    [
        (
            "https://github.com/org/repo/pull/42",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/files",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/commits",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/?tab=checks#discussion_r1",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "http://www.GitHub.com/Org/Repo/pull/42",
            "github.com/org/repo/pull/42",
            "Org/Repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.example.com/Team/Svc/pull/7",
            "github.example.com/team/svc/pull/7",
            "Team/Svc",
            "github.example.com/team/svc",
            7,
        ),
    ],
)
def test_valid_pull_request_urls(
    url: str, normalized_url: str, repo_full_name: str, repo_normalized_url: str, number: int
) -> None:
    ref = parse_pull_request_url(url)
    assert ref is not None
    assert ref.normalized_url == normalized_url
    assert ref.repo_full_name == repo_full_name
    assert ref.repo_normalized_url == repo_normalized_url
    assert ref.number == number
    assert ref.provider == "github"
    assert ref.sources == ()
    assert ref.title == ""


def test_repo_normalized_url_matches_the_remote_normalizer() -> None:
    ref = parse_pull_request_url("https://github.com/Org/Repo/pull/1")
    assert ref is not None
    assert ref.repo_normalized_url == normalize_remote_url("https://github.com/org/repo.git")


@pytest.mark.parametrize(
    "url",
    [
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
    ],
)
def test_invalid_pull_request_urls(url: str) -> None:
    assert parse_pull_request_url(url) is None
