"""Shared helpers for tests that run agents and workers."""

from __future__ import annotations

import sys
from pathlib import Path

from workgraph.app import Workgraph
from workgraph.demo import scripted_agent
from workgraph.config.settings import CommandAgentConfig
from workgraph.jobs.worker import Worker

FAKE_AGENT = str(Path(scripted_agent.__file__))


def enable_fake_agent(app: Workgraph, name: str = "fake", mode: str = "success", *, timeout: float = 120) -> None:
    config = app.config
    config.execution.enabled = True
    config.execution.default_agent = name
    config.agents[name] = CommandAgentConfig(
        executable=sys.executable, args=[FAKE_AGENT, "--mode", mode, "{manifest}"], timeout_seconds=timeout,
        supports_usage=True,
    )
    app.config_store.save_global(config)


def drain(app: Workgraph, timeout: float = 120) -> Worker:
    worker = Worker(app, concurrency=2, poll_interval=0.05, maintenance_interval=0.2)
    worker.run_until_idle(timeout=timeout)
    return worker


def passing_check(name: str = "unit tests") -> dict[str, object]:
    return {"name": name, "kind": "test", "command": [sys.executable, "-c", "print('1 passed')"]}


def failing_check(name: str = "unit tests") -> dict[str, object]:
    code = "print('FAILED tests/test_api.py::test_login - AssertionError'); raise SystemExit(1)"
    return {"name": name, "kind": "test", "command": [sys.executable, "-c", code]}
