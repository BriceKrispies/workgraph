"""Read-only Jira REST client, one flavour per deployment.

``JiraClient`` owns transport concerns: base URL, timeout, retries and error mapping. Every request
goes through ``JiraClient._get``, which sends GET only. A ``JiraFlavour`` owns what differs between
Jira Cloud and Jira Data Center: auth headers, the search request and its pagination, and the REST
API version. See docs/design/tracker-sync.md.
"""

from __future__ import annotations

import base64
import logging
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from datetime import UTC, datetime, tzinfo
from typing import Any, Protocol
from urllib.parse import quote
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

import httpx

from workgraph.core.errors import WorkgraphError

logger = logging.getLogger(__name__)

MAX_ATTEMPTS = 4
"""One request plus up to three retries for 429 and 5xx responses."""
MAX_BACKOFF_SECONDS = 30.0
PAGE_SIZE = 100
DEV_STATUS_PATH = "/rest/dev-status/latest/issue/detail"
Getter = Callable[[str, Mapping[str, str | int]], dict[str, Any]]


class TrackerError(WorkgraphError):
    """A tracker request failed. The message names the path and status, never credentials."""

    code = "tracker_error"
    http_status = 502

    def __init__(self, message: str, *, status: int | None = None) -> None:
        super().__init__(message, details={"status": status} if status is not None else None)
        self.status = status


class JiraFlavour(Protocol):
    """What differs between Jira Cloud and Jira Data Center."""

    name: str
    api_path: str
    """REST API prefix, e.g. ``/rest/api/3``."""
    key_list_fields: str
    """The ``fields`` value that is enough to list ``(id, key)`` pairs."""

    def headers(self) -> dict[str, str]:
        """Authorization headers. Never log or embed them in error messages."""
        ...

    def search(self, get: Getter, jql: str, fields: str) -> Iterator[dict[str, Any]]:
        """Every issue matching ``jql``, across all pages."""
        ...


class CloudFlavour:
    """Jira Cloud: Basic auth with an account email and an API token; token-based search pagination."""

    name = "cloud"
    api_path = "/rest/api/3"
    key_list_fields = "id"

    def __init__(self, email: str, token: str) -> None:
        self._email = email
        self._token = token

    def headers(self) -> dict[str, str]:
        raw = f"{self._email}:{self._token}".encode()
        return {"Authorization": "Basic " + base64.b64encode(raw).decode("ascii")}

    def search(self, get: Getter, jql: str, fields: str) -> Iterator[dict[str, Any]]:
        next_token: str | None = None
        while True:
            params: dict[str, str | int] = {"jql": jql, "fields": fields, "maxResults": PAGE_SIZE}
            if next_token is not None:
                params["nextPageToken"] = next_token
            page = get(f"{self.api_path}/search/jql", params)
            yield from page.get("issues", [])
            next_token = page.get("nextPageToken")
            if page.get("isLast") or not next_token:
                return


class DataCenterFlavour:
    """Jira Data Center: Bearer personal access token; offset pagination.

    Implemented from the REST API documentation but untested against a live Data Center server.
    """

    name = "data_center"
    api_path = "/rest/api/2"
    key_list_fields = "key"

    def __init__(self, token: str) -> None:
        self._token = token

    def headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self._token}"}

    def search(self, get: Getter, jql: str, fields: str) -> Iterator[dict[str, Any]]:
        start = 0
        while True:
            params: dict[str, str | int] = {
                "jql": jql,
                "fields": fields,
                "startAt": start,
                "maxResults": PAGE_SIZE,
            }
            page = get(f"{self.api_path}/search", params)
            issues = page.get("issues", [])
            yield from issues
            start += len(issues)
            if not issues or start >= int(page.get("total", 0)):
                return


class JiraClient:
    """Read-only Jira client. Satisfies ``TrackerClient``."""

    def __init__(
        self,
        base_url: str,
        flavour: JiraFlavour,
        *,
        timeout_seconds: float = 30.0,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.flavour = flavour
        self._sleep = sleep
        self._zone: tzinfo | None = None
        self._http = httpx.Client(
            base_url=base_url.rstrip("/"),
            timeout=timeout_seconds,
            headers={"Accept": "application/json", **flavour.headers()},
            transport=transport,
        )

    def close(self) -> None:
        self._http.close()

    def _user_zone(self) -> tzinfo:
        """The authenticated user's timezone, read once from ``/myself`` and cached.

        JQL reads bare date-times in the user's profile timezone, so ``since`` is formatted in it.
        A request failure propagates and nothing is cached; a missing or unknown zone falls back to UTC.
        """
        if self._zone is None:
            name = self._get(f"{self.flavour.api_path}/myself").get("timeZone")
            zone: tzinfo = UTC
            if not isinstance(name, str) or not name:
                logger.warning("Jira /myself has no timeZone; formatting JQL dates in UTC")
            else:
                try:
                    zone = ZoneInfo(name)
                except (ZoneInfoNotFoundError, ValueError):
                    logger.warning("Jira user timeZone %r is unknown; formatting JQL dates in UTC", name)
            self._zone = zone
        return self._zone

    def _get(self, path: str, params: Mapping[str, str | int] | None = None) -> dict[str, Any]:
        """The only method that sends requests. It sends GET, retries 429 and 5xx, and returns a JSON object."""
        attempt = 1
        while True:
            try:
                response = self._http.get(path, params=params)
            except httpx.TransportError as exc:
                raise TrackerError(f"Jira GET {path} failed: {type(exc).__name__}") from exc
            if not _is_retryable(response.status_code) or attempt >= MAX_ATTEMPTS:
                break
            self._sleep(_retry_delay(response, attempt))
            attempt += 1
        if not response.is_success:
            raise TrackerError(
                f"Jira GET {path} returned HTTP {response.status_code}", status=response.status_code
            )
        try:
            data = response.json()
        except ValueError as exc:
            raise TrackerError(f"Jira GET {path} returned invalid JSON") from exc
        if not isinstance(data, dict):
            raise TrackerError(f"Jira GET {path} returned a JSON value that is not an object")
        return data

    def search_updated(self, projects: Sequence[str], since: datetime | None) -> Iterator[dict[str, Any]]:
        jql = f"project in ({_jql_list(projects)})"
        if since is not None:
            jql += f' AND updated >= "{_jql_minute(since, self._user_zone())}"'
        jql += " ORDER BY updated ASC"
        return self.flavour.search(self._get, jql, "*all")

    def list_keys(self, projects: Sequence[str]) -> Iterator[tuple[str, str]]:
        jql = f"project in ({_jql_list(projects)})"
        for issue in self.flavour.search(self._get, jql, self.flavour.key_list_fields):
            yield str(issue["id"]), str(issue["key"])

    def get_issue(self, key: str) -> dict[str, Any] | None:
        try:
            return self._get(f"{self.flavour.api_path}/issue/{quote(key, safe='')}")
        except TrackerError as exc:
            if exc.status == 404:
                return None
            raise

    def dev_pull_requests(self, external_id: str) -> list[dict[str, Any]]:
        """Pull requests from the development panel. The endpoint is undocumented, so any error yields []."""
        params: dict[str, str | int] = {
            "issueId": external_id,
            "applicationType": "GitHub",
            "dataType": "pullrequest",
        }
        try:
            data = self._get(DEV_STATUS_PATH, params)
            pull_requests: list[dict[str, Any]] = []
            for detail in data.get("detail", []):
                pull_requests.extend(detail.get("pullRequests", []))
            return pull_requests
        except Exception as exc:
            logger.warning("Jira dev-status lookup for issue %s failed: %s", external_id, exc)
            return []


def _is_retryable(status: int) -> bool:
    """429 and 5xx, except 501 (Not Implemented), which will not succeed on retry."""
    return status == 429 or (status >= 500 and status != 501)


def _retry_delay(response: httpx.Response, attempt: int) -> float:
    """``Retry-After`` in seconds when the server sends one, else exponential backoff; both capped."""
    raw = response.headers.get("Retry-After")
    if raw is not None:
        try:
            return min(max(float(raw), 0.0), MAX_BACKOFF_SECONDS)
        except ValueError:
            pass
    return min(0.5 * 2 ** (attempt - 1), MAX_BACKOFF_SECONDS)


def _jql_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _jql_list(projects: Sequence[str]) -> str:
    return ",".join(_jql_string(project) for project in projects)


def _jql_minute(since: datetime, zone: tzinfo) -> str:
    """JQL timestamp in ``zone``, truncated to the minute. A naive ``since`` is taken as UTC."""
    if since.tzinfo is None:
        since = since.replace(tzinfo=UTC)
    return since.astimezone(zone).strftime("%Y-%m-%d %H:%M")
