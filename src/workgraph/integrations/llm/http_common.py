"""Retry policy shared by HTTP-based inference providers."""

from __future__ import annotations

import time
from collections.abc import Callable
from typing import TypeVar

import httpx

from workgraph.core.errors import ProviderUnavailableError

T = TypeVar("T")
RETRYABLE_STATUS = {408, 425, 429, 500, 502, 503, 504}


class RetryableHTTPError(Exception):
    pass


def with_retries(
    fn: Callable[[], T],
    *,
    retries: int,
    backoff_seconds: float,
    provider: str,
    sleep: Callable[[float], None] = time.sleep,
) -> tuple[T, int]:
    """Run ``fn`` with exponential backoff on transport errors and retryable statuses."""
    attempt = 0
    while True:
        attempt += 1
        try:
            return fn(), attempt
        except (httpx.TransportError, RetryableHTTPError) as exc:
            if attempt > retries:
                raise ProviderUnavailableError(
                    f"{provider} is unreachable after {attempt} attempt(s): {exc}",
                    details={"provider": provider, "attempts": attempt},
                ) from exc
            sleep(backoff_seconds * (2 ** (attempt - 1)))


def raise_for_retryable(response: httpx.Response) -> None:
    if response.status_code in RETRYABLE_STATUS:
        raise RetryableHTTPError(f"HTTP {response.status_code}: {response.text[:200]}")
