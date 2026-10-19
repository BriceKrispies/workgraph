"""Maps job resource keys to configured concurrency limits."""

from __future__ import annotations

from typing import TYPE_CHECKING

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.jobs.queue import LimitResolver
from workgraph.persistence.orm import TaskRepository

if TYPE_CHECKING:
    from workgraph.app import Workgraph

GPU = "gpu"
RUN = "run"


def agent_key(name: str) -> str:
    return f"agent:{name}"


def repo_key(repository_id: str) -> str:
    return f"repo:{repository_id}"


def verify_key(repository_id: str) -> str:
    """Bounds the heavy verification jobs running on one repository (``jobs.verify_concurrency``)."""
    return f"verify:{repository_id}"


def tracker_key(connection: str) -> str:
    """Held by the sync of one tracker connection, so at most one sync per connection runs at a time."""
    return f"tracker:{connection}"


def stack_key(stack_id: str, repository_id: str) -> str:
    """Held exclusively by a stack's own integration and verification in one of its repositories."""
    return f"exclusive:stack:{stack_id}:{repository_id}"


def task_verify_keys(session: Session, task_id: str) -> list[str]:
    """``verify:`` keys for every repository the task is linked to, for a plain verification job."""
    return sorted(
        verify_key(repository_id)
        for repository_id in session.scalars(
            select(TaskRepository.repository_id).where(TaskRepository.task_id == task_id)
        )
    )


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
        if key.startswith("verify:"):
            return config.jobs.verify_concurrency
        if key.startswith(("exclusive:", "tracker:")):
            return 1
        return None

    return resolve
