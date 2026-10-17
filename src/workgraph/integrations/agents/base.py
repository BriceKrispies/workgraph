"""The agent provider interface.

Workgraph's domain model never depends on a particular coding agent. A provider
declares what it can do (:class:`AgentCapabilities`) and runs one attempt from an
:class:`AgentRunRequest`, streaming structured :class:`AgentEvent` objects and
returning an :class:`AgentRunResult`.

Telemetry honesty: providers fill ``usage``/``cost_usd`` only from values the agent
actually reported; otherwise they stay empty. ``duration_ms`` measured by Workgraph
is always recorded separately by the runner.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Protocol

RunStatus = Literal["succeeded", "failed", "cancelled", "timed_out"]


@dataclass(frozen=True)
class AgentCapabilities:
    streaming_progress: bool = False
    usage_reporting: bool = False
    cost_reporting: bool = False
    session_resume: bool = False
    isolated_workspace: bool = True
    cancellation: bool = True
    subagents: bool = False
    lifecycle_hooks: bool = False

    def to_dict(self) -> dict[str, bool]:
        return dict(self.__dict__)


@dataclass
class AgentRunRequest:
    attempt_id: str
    task_id: str
    task_key: str
    prompt: str
    workdir: Path
    manifest_path: Path
    manifest: dict[str, Any]
    timeout_seconds: float
    log_path: Path
    env: dict[str, str] = field(default_factory=dict)
    model: str | None = None
    resume_session_id: str | None = None
    additional_dirs: list[Path] = field(default_factory=list)


@dataclass
class AgentEvent:
    kind: Literal[
        "session", "progress", "message", "tool_use", "tool_result", "usage", "log", "artifact", "result"
    ]
    message: str = ""
    data: dict[str, Any] = field(default_factory=dict)


@dataclass
class AgentRunResult:
    status: RunStatus
    exit_code: int | None = None
    summary: str = ""
    error: str | None = None
    session_id: str | None = None
    model: str | None = None
    usage: dict[str, int] = field(default_factory=dict)
    cost_usd: float | None = None
    provider_duration_ms: int | None = None
    num_turns: int | None = None
    artifacts: list[dict[str, Any]] = field(default_factory=list)
    telemetry: dict[str, Any] = field(default_factory=dict)


EventSink = Callable[[AgentEvent], None]
CancelCheck = Callable[[], bool]


class AgentProvider(Protocol):
    name: str
    adapter_type: str

    def capabilities(self) -> AgentCapabilities: ...

    def check_available(self) -> tuple[bool, str]: ...

    def config_snapshot(self) -> dict[str, Any]:
        """Non-secret configuration recorded on each attempt for provenance."""
        ...

    def run(
        self, request: AgentRunRequest, on_event: EventSink, should_cancel: CancelCheck
    ) -> AgentRunResult: ...
