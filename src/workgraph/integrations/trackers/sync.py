"""Mirror tracker issues into Workgraph tasks. Read-only towards the tracker.

Fetching happens outside any write transaction; each page of issues is then applied in a
short transaction as ``system:tracker-sync``. See docs/design/tracker-sync.md.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import TYPE_CHECKING, Any

from workgraph.config.settings import TrackerConfig
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError
from workgraph.core.services.base import Service
from workgraph.integrations.trackers.types import SyncReport, TrackerClient, TrackerSnapshot

if TYPE_CHECKING:
    from workgraph.app import Workgraph

ClientFactory = Callable[[TrackerConfig], TrackerClient]
Mapper = Callable[[dict[str, Any], list[dict[str, Any]], TrackerConfig], TrackerSnapshot]


def default_client_factory(config: TrackerConfig) -> TrackerClient:
    from workgraph.integrations.trackers.jira import make_jira_client

    return make_jira_client(config)


def default_mapper(
    issue: dict[str, Any], dev_pull_requests: list[dict[str, Any]], config: TrackerConfig
) -> TrackerSnapshot:
    from workgraph.integrations.trackers.mapper import map_issue

    return map_issue(issue, dev_pull_requests, config)


class TrackerSyncService(Service):
    def __init__(
        self,
        app: Workgraph,
        *,
        client_factory: ClientFactory | None = None,
        mapper: Mapper | None = None,
    ) -> None:
        super().__init__(app)
        self.client_factory = client_factory or default_client_factory
        self.mapper = mapper or default_mapper

    def connection(self, name: str) -> TrackerConfig:
        found = self.app.config.trackers.get(name)
        if found is None:
            raise NotFoundError(
                f"tracker connection {name!r} is not configured", details={"connection": name}
            )
        return found

    def sync(self, ctx: RequestContext, connection: str, *, full: bool = False) -> SyncReport:
        """Fetch issues changed since the connection's cursor (every issue when ``full``) and apply them."""
        raise NotImplementedError("WG-1672")

    def status(self, ctx: RequestContext, connection: str) -> dict[str, Any]:
        """Cursor, last sync times, tracked task counts and unresolved pull request repositories."""
        raise NotImplementedError("WG-1672")
