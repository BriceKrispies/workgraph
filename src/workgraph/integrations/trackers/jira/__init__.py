"""Jira tracker client factory. Read-only; see docs/design/tracker-sync.md."""

from __future__ import annotations

import os
from collections.abc import Mapping

import httpx

from workgraph.config.settings import TrackerConfig
from workgraph.core.errors import ConfigError
from workgraph.integrations.trackers.jira.client import (
    CloudFlavour,
    DataCenterFlavour,
    JiraClient,
    JiraFlavour,
)
from workgraph.integrations.trackers.types import TrackerClient

__all__ = ["make_jira_client"]


def make_jira_client(
    config: TrackerConfig,
    *,
    env: Mapping[str, str] | None = None,
    transport: httpx.BaseTransport | None = None,
) -> TrackerClient:
    """A read-only client for ``config``. Credentials come only from the variables it names."""
    source: Mapping[str, str] = os.environ if env is None else env
    token = _credential(source, config.token_env)
    flavour: JiraFlavour
    if config.flavour == "cloud":
        if config.email_env is None:
            raise ConfigError("Jira Cloud needs email_env: the variable holding the account email")
        flavour = CloudFlavour(_credential(source, config.email_env), token)
    else:
        flavour = DataCenterFlavour(token)
    return JiraClient(
        config.base_url,
        flavour,
        timeout_seconds=config.timeout_seconds,
        transport=transport,
    )


def _credential(source: Mapping[str, str], name: str) -> str:
    value = source.get(name)
    if not value:
        raise ConfigError(
            f"environment variable {name} is not set; Jira credentials are read from it",
            details={"env": name},
        )
    return value
