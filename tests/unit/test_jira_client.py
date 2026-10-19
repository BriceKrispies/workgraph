"""Jira client: pagination, JQL, auth, retries, error mapping, and the read-only guarantee."""

from __future__ import annotations

import base64
import logging
from collections.abc import Callable
from datetime import UTC, datetime, timedelta, timezone
from typing import Any

import httpx
import pytest

from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.errors import ConfigError
from workgraph.integrations.trackers.jira import make_jira_client
from workgraph.integrations.trackers.jira.client import (
    CloudFlavour,
    JiraClient,
    TrackerError,
)

EMAIL = "bot@example.com"
TOKEN = "s3cret-token"
ENV = {"WG_JIRA_EMAIL": EMAIL, "WG_JIRA_TOKEN": TOKEN}
SEEN: list[httpx.Request] = []


@pytest.fixture(autouse=True)
def _every_request_was_get():
    SEEN.clear()
    yield
    assert all(request.method == "GET" for request in SEEN)


def recording(handler: Callable[[httpx.Request], httpx.Response]) -> httpx.MockTransport:
    def wrapped(request: httpx.Request) -> httpx.Response:
        SEEN.append(request)
        return handler(request)

    return httpx.MockTransport(wrapped)


def cloud_config(**overrides: Any) -> JiraTrackerConfig:
    values: dict[str, Any] = {
        "base_url": "https://acme.atlassian.net",
        "workspace": "work",
        "projects": ["ABC"],
        "email_env": "WG_JIRA_EMAIL",
        "token_env": "WG_JIRA_TOKEN",
    }
    values.update(overrides)
    return JiraTrackerConfig(**values)


def dc_config(**overrides: Any) -> JiraTrackerConfig:
    return cloud_config(flavour="data_center", email_env=None, **overrides)


def issue(key: str, number: int) -> dict[str, object]:
    return {"id": str(number), "key": key, "fields": {"summary": key}}


def test_cloud_search_follows_next_page_token():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.params.get("nextPageToken") is None:
            return httpx.Response(
                200, json={"issues": [issue("ABC-1", 1)], "nextPageToken": "tok2", "isLast": False}
            )
        return httpx.Response(200, json={"issues": [issue("ABC-2", 2)], "isLast": True})

    client = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    keys = [item["key"] for item in client.search_updated(["ABC"], None)]

    assert keys == ["ABC-1", "ABC-2"]
    assert [request.url.path for request in SEEN] == ["/rest/api/3/search/jql"] * 2
    first, second = SEEN[0].url.params, SEEN[1].url.params
    assert first.get("fields") == "*all"
    assert first.get("maxResults") == "100"
    assert first.get("nextPageToken") is None
    assert second.get("nextPageToken") == "tok2"
    client.close()


def test_cloud_list_keys_requests_id_fields():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"issues": [issue("ABC-1", 10001)], "isLast": True})

    client = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    assert list(client.list_keys(["ABC"])) == [("10001", "ABC-1")]
    assert SEEN[0].url.params.get("fields") == "id"
    client.close()


def test_data_center_search_pages_by_start_at():
    pages = {"0": (["ABC-1", "ABC-2"], 3), "2": (["ABC-3"], 3)}

    def handler(request: httpx.Request) -> httpx.Response:
        keys, total = pages[request.url.params["startAt"]]
        return httpx.Response(200, json={"startAt": 0, "total": total, "issues": [issue(k, 0) for k in keys]})

    client = make_jira_client(dc_config(), env=ENV, transport=recording(handler))
    keys = [item["key"] for item in client.search_updated(["ABC"], None)]

    assert keys == ["ABC-1", "ABC-2", "ABC-3"]
    assert [request.url.path for request in SEEN] == ["/rest/api/2/search"] * 2
    assert [request.url.params["startAt"] for request in SEEN] == ["0", "2"]
    client.close()


def test_jql_without_since_has_no_updated_clause():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"issues": [], "isLast": True})

    client = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    assert list(client.search_updated(["ABC", "DEF"], None)) == []
    assert SEEN[0].url.params["jql"] == 'project in ("ABC","DEF") ORDER BY updated ASC'
    client.close()


def myself_handler(time_zone: object | None):
    """Answers ``/myself`` with ``timeZone`` (omitted when None) and every search with no issues."""

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/myself"):
            body: dict[str, object] = {"accountId": "abc"}
            if time_zone is not None:
                body["timeZone"] = time_zone
            return httpx.Response(200, json=body)
        return httpx.Response(200, json={"issues": [], "isLast": True})

    return handler


def test_jql_with_since_uses_the_user_timezone_and_caches_it():
    client = make_jira_client(cloud_config(), env=ENV, transport=recording(myself_handler("America/Chicago")))
    # 2026-10-18 is CDT (UTC-5): 09:30:45 UTC and 11:30 at UTC+2 are both 04:30 in Chicago.
    list(client.search_updated(["ABC", "DEF"], datetime(2026, 10, 18, 9, 30, 45, tzinfo=UTC)))
    list(client.search_updated(["ABC"], datetime(2026, 10, 18, 11, 30, tzinfo=timezone(timedelta(hours=2)))))

    myself = [r for r in SEEN if r.url.path == "/rest/api/3/myself"]
    assert len(myself) == 1
    searches = [r for r in SEEN if r.url.path == "/rest/api/3/search/jql"]
    assert searches[0].url.params["jql"] == (
        'project in ("ABC","DEF") AND updated >= "2026-10-18 04:30" ORDER BY updated ASC'
    )
    assert searches[1].url.params["jql"] == (
        'project in ("ABC") AND updated >= "2026-10-18 04:30" ORDER BY updated ASC'
    )
    client.close()


def test_missing_timezone_falls_back_to_utc_with_warning(caplog: pytest.LogCaptureFixture):
    client = make_jira_client(cloud_config(), env=ENV, transport=recording(myself_handler(None)))
    with caplog.at_level(logging.WARNING):
        list(client.search_updated(["ABC"], datetime(2026, 10, 18, 9, 30, tzinfo=UTC)))
    assert (
        SEEN[-1].url.params["jql"]
        == 'project in ("ABC") AND updated >= "2026-10-18 09:30" ORDER BY updated ASC'
    )
    assert "timeZone" in caplog.text
    client.close()


def test_unknown_timezone_falls_back_to_utc_with_warning(caplog: pytest.LogCaptureFixture):
    client = make_jira_client(
        cloud_config(), env=ENV, transport=recording(myself_handler("Mars/Olympus_Mons"))
    )
    with caplog.at_level(logging.WARNING):
        list(client.search_updated(["ABC"], datetime(2026, 10, 18, 9, 30, tzinfo=UTC)))
    assert (
        SEEN[-1].url.params["jql"]
        == 'project in ("ABC") AND updated >= "2026-10-18 09:30" ORDER BY updated ASC'
    )
    assert "Mars/Olympus_Mons" in caplog.text
    client.close()


def test_data_center_reads_its_own_myself_endpoint():
    client = make_jira_client(dc_config(), env=ENV, transport=recording(myself_handler("Europe/Paris")))
    list(client.search_updated(["ABC"], datetime(2026, 10, 18, 9, 30, tzinfo=UTC)))
    assert SEEN[0].url.path == "/rest/api/2/myself"
    # Paris is CEST (UTC+2) in October.
    assert (
        SEEN[1].url.params["jql"]
        == 'project in ("ABC") AND updated >= "2026-10-18 11:30" ORDER BY updated ASC'
    )
    client.close()


def test_cloud_uses_basic_auth_and_data_center_uses_bearer():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"id": "1", "key": "ABC-1", "fields": {}})

    cloud = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    cloud.get_issue("ABC-1")
    expected = "Basic " + base64.b64encode(f"{EMAIL}:{TOKEN}".encode()).decode()
    assert SEEN[-1].headers["Authorization"] == expected
    assert SEEN[-1].headers["Accept"] == "application/json"
    cloud.close()

    dc = make_jira_client(dc_config(), env=ENV, transport=recording(handler))
    dc.get_issue("ABC-1")
    assert SEEN[-1].headers["Authorization"] == f"Bearer {TOKEN}"
    assert SEEN[-1].url.path == "/rest/api/2/issue/ABC-1"
    dc.close()


def test_429_is_retried_after_retry_after_seconds():
    responses = [
        httpx.Response(429, headers={"Retry-After": "7"}),
        httpx.Response(200, json={"id": "1", "key": "ABC-1", "fields": {}}),
    ]
    delays: list[float] = []
    client = JiraClient(
        "https://acme.atlassian.net",
        CloudFlavour(EMAIL, TOKEN),
        transport=recording(lambda request: responses.pop(0)),
        sleep=delays.append,
    )

    issue_json = client.get_issue("ABC-1")

    assert issue_json is not None and issue_json["key"] == "ABC-1"
    assert delays == [7.0]
    assert len(SEEN) == 2
    client.close()


def test_501_is_not_retried():
    delays: list[float] = []
    client = JiraClient(
        "https://acme.atlassian.net",
        CloudFlavour(EMAIL, TOKEN),
        transport=recording(lambda request: httpx.Response(501)),
        sleep=delays.append,
    )

    with pytest.raises(TrackerError) as raised:
        client.get_issue("ABC-1")

    assert raised.value.status == 501
    assert len(SEEN) == 1
    assert delays == []
    client.close()


def test_persistent_500_raises_tracker_error_without_credentials():
    delays: list[float] = []
    client = JiraClient(
        "https://acme.atlassian.net",
        CloudFlavour(EMAIL, TOKEN),
        transport=recording(lambda request: httpx.Response(500, text="boom")),
        sleep=delays.append,
    )

    with pytest.raises(TrackerError) as raised:
        client.get_issue("ABC-1")

    message = raised.value.message
    assert "500" in message and "/rest/api/3/issue/ABC-1" in message
    for secret in (TOKEN, EMAIL, "Basic ", base64.b64encode(f"{EMAIL}:{TOKEN}".encode()).decode()):
        assert secret not in message
    assert len(SEEN) == 4
    assert len(delays) == 3
    client.close()


def test_get_issue_404_returns_none():
    client = make_jira_client(
        cloud_config(),
        env=ENV,
        transport=recording(lambda request: httpx.Response(404, json={"errorMessages": ["gone"]})),
    )
    assert client.get_issue("ABC-404") is None
    client.close()


def test_dev_pull_requests_flattens_details():
    body = {
        "detail": [
            {"pullRequests": [{"url": "https://github.com/acme/a/pull/1"}]},
            {"pullRequests": [{"url": "https://github.com/acme/b/pull/2"}, {"url": "c"}]},
            {},
        ]
    }

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=body)

    client = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    pulls = client.dev_pull_requests("10001")

    assert [pull["url"] for pull in pulls] == [
        "https://github.com/acme/a/pull/1",
        "https://github.com/acme/b/pull/2",
        "c",
    ]
    request = SEEN[0]
    assert request.url.path == "/rest/dev-status/latest/issue/detail"
    assert request.url.params["issueId"] == "10001"
    assert request.url.params["applicationType"] == "GitHub"
    assert request.url.params["dataType"] == "pullrequest"
    client.close()


def test_dev_pull_requests_error_returns_empty_and_logs(caplog: pytest.LogCaptureFixture):
    client = make_jira_client(
        cloud_config(),
        env=ENV,
        transport=recording(lambda request: httpx.Response(403)),
    )
    with caplog.at_level(logging.WARNING):
        assert client.dev_pull_requests("10001") == []
    assert "dev-status" in caplog.text
    client.close()


def test_missing_token_variable_raises_config_error_naming_it():
    with pytest.raises(ConfigError) as raised:
        make_jira_client(cloud_config(), env={"WG_JIRA_EMAIL": EMAIL})
    assert "WG_JIRA_TOKEN" in raised.value.message
    assert EMAIL not in raised.value.message


def test_missing_email_variable_raises_config_error_naming_it():
    with pytest.raises(ConfigError) as raised:
        make_jira_client(cloud_config(), env={"WG_JIRA_TOKEN": TOKEN})
    assert "WG_JIRA_EMAIL" in raised.value.message
    assert TOKEN not in raised.value.message


def test_every_method_of_the_client_sends_only_get():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.startswith("/rest/dev-status"):
            return httpx.Response(200, json={"detail": []})
        if request.url.path.endswith("/myself"):
            return httpx.Response(200, json={"timeZone": "UTC"})
        if "/issue/" in request.url.path:
            return httpx.Response(200, json={"id": "1", "key": "ABC-1", "fields": {}})
        return httpx.Response(200, json={"issues": [issue("ABC-1", 1)], "isLast": True})

    client = make_jira_client(cloud_config(), env=ENV, transport=recording(handler))
    list(client.search_updated(["ABC"], datetime(2026, 1, 1, tzinfo=UTC)))
    list(client.list_keys(["ABC"]))
    client.get_issue("ABC-1")
    client.dev_pull_requests("1")
    client.close()

    assert len(SEEN) == 5
    assert {request.method for request in SEEN} == {"GET"}
    assert not any(hasattr(JiraClient, name) for name in ("post", "put", "patch", "delete"))
