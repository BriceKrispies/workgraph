"""Inference provider registry built from configuration."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from workgraph.config.settings import (
    HeuristicProviderConfig,
    ModelSelection,
    OllamaProviderConfig,
    OpenAICompatibleProviderConfig,
)
from workgraph.core.errors import NotFoundError
from workgraph.integrations.llm.base import LLMProvider
from workgraph.integrations.llm.heuristic import HeuristicProvider
from workgraph.integrations.llm.ollama import OllamaProvider
from workgraph.integrations.llm.openai_compatible import OpenAICompatibleProvider

if TYPE_CHECKING:
    from workgraph.app import Workgraph

# Extension point: tests or plugins may register additional provider factories.
EXTRA_FACTORIES: dict[str, Any] = {}


def build(
    name: str, config: OllamaProviderConfig | HeuristicProviderConfig | OpenAICompatibleProviderConfig
) -> LLMProvider:
    if name in EXTRA_FACTORIES:
        return EXTRA_FACTORIES[name](name, config)
    if isinstance(config, OllamaProviderConfig):
        return OllamaProvider(name, config)
    if isinstance(config, OpenAICompatibleProviderConfig):
        return OpenAICompatibleProvider(name, config)
    return HeuristicProvider(name)


class LLMRegistry:
    def __init__(self, app: Workgraph) -> None:
        self.app = app

    def get(self, name: str, workspace_slug: str | None = None) -> LLMProvider:
        providers = self.app.effective_config(workspace_slug).llm.providers
        if name not in providers:
            raise NotFoundError(
                f"inference provider {name!r} is not configured", details={"configured": sorted(providers)}
            )
        return build(name, providers[name])

    def selection(self, capability: str, workspace_slug: str | None = None) -> ModelSelection:
        llm = self.app.effective_config(workspace_slug).llm
        return {
            "classification": llm.classification,
            "embeddings": llm.embeddings,
            "summarization": llm.summarization,
        }[capability]

    def health(self) -> dict[str, dict[str, Any]]:
        config = self.app.config.llm
        report: dict[str, dict[str, Any]] = {}
        for name in config.providers:
            provider = self.get(name)
            wanted = [
                s.model
                for s in (config.classification, config.embeddings, config.summarization)
                if s.provider == name
            ]
            if not wanted:
                try:
                    models = provider.list_models()
                    report[name] = {"available": True, "detail": f"{len(models)} models"}
                except Exception as exc:
                    report[name] = {"available": False, "detail": str(exc)}
                continue
            details = []
            ok_all = True
            for model in dict.fromkeys(wanted):
                ok, detail = provider.check_model(model)
                ok_all = ok_all and ok
                details.append(detail)
            report[name] = {"available": ok_all, "detail": "; ".join(details)}
        return report
