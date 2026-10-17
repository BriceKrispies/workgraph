"""OpenAI-compatible HTTP provider (llama.cpp server, vLLM, LM Studio, ...)."""

from __future__ import annotations

import json
import time
from typing import Any

import httpx

from workgraph.config.settings import OpenAICompatibleProviderConfig, secret_from_env
from workgraph.core.errors import ProviderUnavailableError
from workgraph.integrations.llm.base import (
    InvalidModelOutputError,
    ModelUnavailableError,
    ProviderCapabilities,
    StructuredResponse,
)
from workgraph.integrations.llm.http_common import raise_for_retryable, with_retries


class OpenAICompatibleProvider:
    kind = "openai_compatible"

    def __init__(
        self,
        name: str,
        config: OpenAICompatibleProviderConfig,
        *,
        transport: httpx.BaseTransport | None = None,
    ) -> None:
        self.name = name
        self.config = config
        self._transport = transport

    def _client(self) -> httpx.Client:
        headers = {}
        key = secret_from_env(self.config.api_key_env)
        if key:
            headers["Authorization"] = f"Bearer {key}"
        return httpx.Client(
            base_url=self.config.base_url.rstrip("/"),
            timeout=self.config.timeout_seconds,
            headers=headers,
            transport=self._transport,
        )

    def capabilities(self) -> ProviderCapabilities:
        return ProviderCapabilities(structured_output=True, embeddings=True, local=True)

    def _call(self, method: str, path: str, body: dict[str, Any] | None = None) -> tuple[dict[str, Any], int]:
        def call() -> dict[str, Any]:
            with self._client() as client:
                response = client.request(method, path, json=body)
                raise_for_retryable(response)
                if response.status_code == 404:
                    raise ModelUnavailableError(f"{self.name}: {response.text[:200]}")
                if response.status_code >= 400:
                    raise InvalidModelOutputError(
                        f"{self.name} error {response.status_code}: {response.text[:300]}"
                    )
                return response.json()

        return with_retries(
            call,
            retries=self.config.max_retries,
            backoff_seconds=self.config.backoff_seconds,
            provider=f"{self.name} at {self.config.base_url}",
        )

    def list_models(self) -> list[str]:
        data, _ = self._call("GET", "/v1/models")
        return sorted(str(m.get("id")) for m in data.get("data", []))

    def check_model(self, model: str) -> tuple[bool, str]:
        try:
            models = self.list_models()
        except ProviderUnavailableError as exc:
            return False, exc.message
        return (model in models, f"{model} available" if model in models else f"{model!r} not served")

    def generate_json(
        self, model: str, *, system: str, prompt: str, schema: dict[str, Any], temperature: float = 0.0
    ) -> StructuredResponse:
        started = time.monotonic()
        body = {
            "model": model,
            "temperature": temperature,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": prompt}],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": schema.get("title", "result"), "schema": schema, "strict": True},
            },
        }
        data, attempts = self._call("POST", "/v1/chat/completions", body)
        try:
            content = data["choices"][0]["message"]["content"]
            parsed = json.loads(content)
        except (KeyError, IndexError, TypeError, json.JSONDecodeError) as exc:
            raise InvalidModelOutputError(f"unexpected completion payload: {str(data)[:200]}") from exc
        if not isinstance(parsed, dict):
            raise InvalidModelOutputError("model output is not a JSON object")
        return StructuredResponse(
            data=parsed,
            raw_text=content,
            model=str(data.get("model") or model),
            duration_ms=int((time.monotonic() - started) * 1000),
            attempts=attempts,
        )

    def embed(self, model: str, texts: list[str]) -> list[list[float]]:
        data, _ = self._call("POST", "/v1/embeddings", {"model": model, "input": texts})
        items = sorted(data.get("data", []), key=lambda d: d.get("index", 0))
        if len(items) != len(texts):
            raise InvalidModelOutputError("embedding count mismatch")
        return [[float(x) for x in item["embedding"]] for item in items]
