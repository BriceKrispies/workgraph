"""Ollama provider (HTTP API): JSON-Schema constrained chat and embeddings."""

from __future__ import annotations

import json
import time
from typing import Any

import httpx

from workgraph.config.settings import OllamaProviderConfig, secret_from_env
from workgraph.core.errors import ProviderUnavailableError
from workgraph.integrations.llm.base import (
    InvalidModelOutputError,
    ModelUnavailableError,
    ProviderCapabilities,
    StructuredResponse,
)
from workgraph.integrations.llm.http_common import raise_for_retryable, with_retries


class OllamaProvider:
    kind = "ollama"

    def __init__(
        self, name: str, config: OllamaProviderConfig, *, transport: httpx.BaseTransport | None = None
    ) -> None:
        self.name = name
        self.config = config
        self._transport = transport

    def _client(self, timeout: float | None = None) -> httpx.Client:
        headers = {}
        key = secret_from_env(self.config.api_key_env)
        if key:
            headers["Authorization"] = f"Bearer {key}"
        return httpx.Client(
            base_url=self.config.base_url.rstrip("/"),
            timeout=timeout or self.config.timeout_seconds,
            headers=headers,
            transport=self._transport,
        )

    def capabilities(self) -> ProviderCapabilities:
        return ProviderCapabilities(structured_output=True, embeddings=True, local=True)

    def list_models(self) -> list[str]:
        def call() -> list[str]:
            with self._client(timeout=10) as client:
                response = client.get("/api/tags")
                raise_for_retryable(response)
                response.raise_for_status()
                return sorted(m["name"] for m in response.json().get("models", []))

        try:
            models, _ = with_retries(
                call, retries=0, backoff_seconds=0, provider=f"ollama at {self.config.base_url}"
            )
        except httpx.HTTPStatusError as exc:
            raise ProviderUnavailableError(f"ollama returned {exc.response.status_code}") from exc
        return models

    def check_model(self, model: str) -> tuple[bool, str]:
        try:
            models = self.list_models()
        except ProviderUnavailableError as exc:
            return False, exc.message
        names = set(models) | {m.removesuffix(":latest") for m in models}
        if model in names or f"{model}:latest" in models:
            return True, f"{model} available"
        return (
            False,
            f"model {model!r} is not pulled (available: {', '.join(models[:10]) or 'none'}); run `ollama pull {model}`",
        )

    def _post(self, path: str, body: dict[str, Any]) -> tuple[dict[str, Any], int]:
        def call() -> dict[str, Any]:
            with self._client() as client:
                response = client.post(path, json=body)
                raise_for_retryable(response)
                if response.status_code == 404:
                    message = response.json().get("error", "") if response.content else ""
                    raise ModelUnavailableError(
                        f"ollama model {body.get('model')!r} is unavailable: {message or 'not found'}",
                        details={"model": body.get("model")},
                    )
                if response.status_code >= 400:
                    raise InvalidModelOutputError(
                        f"ollama error {response.status_code}: {response.text[:300]}"
                    )
                return response.json()

        return with_retries(
            call,
            retries=self.config.max_retries,
            backoff_seconds=self.config.backoff_seconds,
            provider=f"ollama at {self.config.base_url}",
        )

    def generate_json(
        self, model: str, *, system: str, prompt: str, schema: dict[str, Any], temperature: float = 0.0
    ) -> StructuredResponse:
        started = time.monotonic()
        body = {
            "model": model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": prompt}],
            "format": schema,
            "stream": False,
            "options": {"temperature": temperature},
        }
        data, attempts = self._post("/api/chat", body)
        content = (data.get("message") or {}).get("content", "")
        try:
            parsed = json.loads(content)
        except json.JSONDecodeError as exc:
            raise InvalidModelOutputError(f"model returned non-JSON output: {content[:200]!r}") from exc
        if not isinstance(parsed, dict):
            raise InvalidModelOutputError("model output is not a JSON object")
        return StructuredResponse(
            data=parsed,
            raw_text=content,
            model=str(data.get("model") or model),
            duration_ms=int((time.monotonic() - started) * 1000),
            attempts=attempts,
            metadata={
                k: data.get(k) for k in ("total_duration", "eval_count", "prompt_eval_count") if k in data
            },
        )

    def embed(self, model: str, texts: list[str]) -> list[list[float]]:
        data, _ = self._post("/api/embed", {"model": model, "input": texts})
        vectors = data.get("embeddings")
        if not isinstance(vectors, list) or len(vectors) != len(texts):
            raise InvalidModelOutputError("ollama returned an unexpected embedding payload")
        return [[float(x) for x in vec] for vec in vectors]
