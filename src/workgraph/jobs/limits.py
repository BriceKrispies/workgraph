"""Maps job resource keys to configured concurrency limits."""

from __future__ import annotations

from typing import TYPE_CHECKING

from workgraph.jobs.queue import LimitResolver

if TYPE_CHECKING:
    from workgraph.app import Workgraph

GPU = "gpu"
RUN = "run"


def agent_key(name: str) -> str:
    return f"agent:{name}"


def repo_key(repository_id: str) -> str:
    return f"repo:{repository_id}"


def resource_limit_resolver(app: Workgraph) -> LimitResolver:
    def resolve(key: str) -> int | None:
        config = app.config
        if key == GPU:
            return config.jobs.gpu_concurrency
        if key == RUN:
            return config.execution.max_concurrent_runs
        if key.startswith("agent:"):
            return config.jobs.per_agent_concurrency
        if key.startswith("repo:"):
            return config.jobs.per_repository_concurrency
        if key.startswith("exclusive:"):
            return 1
        return None

    return resolve
