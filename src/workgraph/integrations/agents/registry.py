"""Agent provider registry built from configuration."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from workgraph.config.settings import ClaudeCodeAgentConfig, CommandAgentConfig
from workgraph.core.errors import NotFoundError, ProviderUnavailableError
from workgraph.integrations.agents.base import AgentProvider
from workgraph.integrations.agents.command import CommandAgentProvider
from workgraph.integrations.claude_code.adapter import ClaudeCodeProvider

if TYPE_CHECKING:
    from workgraph.app import Workgraph


def build_provider(name: str, config: ClaudeCodeAgentConfig | CommandAgentConfig) -> AgentProvider:
    if isinstance(config, ClaudeCodeAgentConfig):
        return ClaudeCodeProvider(name, config)
    return CommandAgentProvider(name, config)


class AgentRegistry:
    def __init__(self, app: Workgraph) -> None:
        self.app = app

    def providers(self, workspace_slug: str | None = None) -> dict[str, AgentProvider]:
        config = self.app.effective_config(workspace_slug)
        return {name: build_provider(name, cfg) for name, cfg in config.agents.items()}

    def get(self, name: str, workspace_slug: str | None = None) -> AgentProvider:
        providers = self.providers(workspace_slug)
        if name not in providers:
            raise NotFoundError(
                f"agent adapter {name!r} is not configured",
                details={"configured": sorted(providers)},
            )
        return providers[name]

    def require_available(self, name: str, workspace_slug: str | None = None) -> AgentProvider:
        provider = self.get(name, workspace_slug)
        ok, detail = provider.check_available()
        if not ok:
            raise ProviderUnavailableError(f"agent adapter {name!r} is unavailable: {detail}")
        return provider

    def describe(self, workspace_slug: str | None = None) -> list[dict[str, Any]]:
        out = []
        for name, provider in self.providers(workspace_slug).items():
            ok, detail = provider.check_available()
            out.append(
                {
                    "name": name,
                    "adapter": provider.adapter_type,
                    "available": ok,
                    "detail": detail,
                    "capabilities": provider.capabilities().to_dict(),
                    "config": provider.config_snapshot(),
                }
            )
        return out

    def health(self) -> dict[str, dict[str, Any]]:
        return {d["name"]: {"available": d["available"], "detail": d["detail"]} for d in self.describe()}
