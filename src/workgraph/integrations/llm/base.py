"""Inference provider interface.

Classification (structured generation) and embeddings are separate capabilities;
a provider may support either or both. Model-dependent work always runs in
background jobs, so an unavailable provider surfaces as a failed/retried job and
never blocks ordinary task tracking.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Protocol

from workgraph.core.errors import ProviderUnavailableError, WorkgraphError


class ModelUnavailableError(ProviderUnavailableError):
    code = "model_unavailable"


class InvalidModelOutputError(WorkgraphError):
    code = "invalid_model_output"
    http_status = 502


class CapabilityNotSupportedError(WorkgraphError):
    code = "capability_not_supported"


@dataclass
class StructuredResponse:
    data: dict[str, Any]
    raw_text: str
    model: str
    duration_ms: int
    attempts: int = 1
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class ProviderCapabilities:
    structured_output: bool
    embeddings: bool
    local: bool
    deterministic: bool = False


class LLMProvider(Protocol):
    name: str
    kind: str

    def capabilities(self) -> ProviderCapabilities: ...

    def list_models(self) -> list[str]: ...

    def check_model(self, model: str) -> tuple[bool, str]: ...

    def generate_json(
        self, model: str, *, system: str, prompt: str, schema: dict[str, Any], temperature: float = 0.0
    ) -> StructuredResponse: ...

    def embed(self, model: str, texts: list[str]) -> list[list[float]]: ...
