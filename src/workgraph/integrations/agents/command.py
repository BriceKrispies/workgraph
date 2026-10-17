"""Generic command agent adapter (the ``workgraph.agent.v1`` JSON-lines protocol).

Workgraph runs an explicitly configured executable with an argument template. No
shell is involved: every template element becomes exactly one argument after
placeholder substitution. Allowed placeholders::

    {attempt_id} {task_id} {task_key} {workdir} {manifest} {prompt_file}

Input: a JSON request on stdin (also written to ``workgraph-request.json`` next to the manifest)::

    {"protocol": "workgraph.agent.v1", "attempt_id": ..., "task": {...},
     "prompt": "...", "manifest": {...}}

Output: JSON objects, one per stdout line. Recognised ``type`` values:

* ``progress`` - ``{"message": str, "percent": number?}``
* ``log``      - ``{"message": str}``
* ``usage``    - ``{"input_tokens": int, "output_tokens": int, ...}`` (measured only)
* ``artifact`` - ``{"name": str, "path": str, "kind": str?}``
* ``result``   - ``{"status": "succeeded"|"failed", "summary": str, "cost_usd": number?}``

Non-JSON lines are kept as log output. Exit code 0 without a ``result`` line is a
success with an empty summary; any other exit code is a failure.
"""

from __future__ import annotations

import json
import re
from typing import Any

from workgraph.config.settings import CommandAgentConfig
from workgraph.core.errors import ConfigError, ProviderUnavailableError
from workgraph.integrations.agents.base import (
    AgentCapabilities,
    AgentEvent,
    AgentRunRequest,
    AgentRunResult,
    CancelCheck,
    EventSink,
)
from workgraph.integrations.process import base_environment, resolve_executable, run_process

PROTOCOL = "workgraph.agent.v1"
PLACEHOLDERS = {"attempt_id", "task_id", "task_key", "workdir", "manifest", "prompt_file"}
_PLACEHOLDER_RE = re.compile(r"\{([^{}]*)\}")


def validate_template(args: list[str]) -> None:
    for arg in args:
        for name in _PLACEHOLDER_RE.findall(arg):
            if name not in PLACEHOLDERS:
                raise ConfigError(
                    f"unknown placeholder {{{name}}} in agent argument template",
                    details={"allowed": sorted(PLACEHOLDERS)},
                )


def render_args(args: list[str], values: dict[str, str]) -> list[str]:
    def substitute(arg: str) -> str:
        return _PLACEHOLDER_RE.sub(lambda m: values[m.group(1)], arg)

    return [substitute(a) for a in args]


class CommandAgentProvider:
    adapter_type = "command"

    def __init__(self, name: str, config: CommandAgentConfig) -> None:
        validate_template(config.args)
        self.name = name
        self.config = config

    def capabilities(self) -> AgentCapabilities:
        return AgentCapabilities(
            streaming_progress=self.config.supports_progress,
            usage_reporting=self.config.supports_usage,
            session_resume=self.config.supports_resume,
            isolated_workspace=True,
            cancellation=True,
        )

    def check_available(self) -> tuple[bool, str]:
        try:
            return True, resolve_executable(self.config.executable)
        except ProviderUnavailableError as exc:
            return False, exc.message

    def config_snapshot(self) -> dict[str, Any]:
        return {
            "adapter": self.adapter_type,
            "executable": self.config.executable,
            "args": self.config.args,
            "env_keys": sorted(self.config.env),
            "timeout_seconds": self.config.timeout_seconds,
        }

    def run(
        self, request: AgentRunRequest, on_event: EventSink, should_cancel: CancelCheck
    ) -> AgentRunResult:
        # Protocol files live beside the manifest (the attempt directory), never inside a repository.
        control_dir = request.manifest_path.parent
        prompt_file = control_dir / "workgraph-prompt.md"
        prompt_file.write_text(request.prompt, encoding="utf-8")
        payload = {
            "protocol": PROTOCOL,
            "attempt_id": request.attempt_id,
            "task": {"id": request.task_id, "key": request.task_key},
            "prompt": request.prompt,
            "manifest": request.manifest,
        }
        (control_dir / "workgraph-request.json").write_text(json.dumps(payload, indent=2), encoding="utf-8")
        values = {
            "attempt_id": request.attempt_id,
            "task_id": request.task_id,
            "task_key": request.task_key,
            "workdir": str(request.workdir),
            "manifest": str(request.manifest_path),
            "prompt_file": str(prompt_file),
        }
        argv = [self.config.executable, *render_args(self.config.args, values)]
        state: dict[str, Any] = {"result": None, "usage": {}, "artifacts": [], "progress": 0, "logs": 0}

        def on_line(line: str) -> None:
            text = line.strip()
            if not text.startswith("{"):
                state["logs"] += 1
                on_event(AgentEvent("log", text[:2000]))
                return
            try:
                message = json.loads(text)
            except json.JSONDecodeError:
                state["logs"] += 1
                on_event(AgentEvent("log", text[:2000]))
                return
            kind = message.get("type")
            if kind == "progress":
                state["progress"] += 1
                on_event(
                    AgentEvent(
                        "progress",
                        str(message.get("message", ""))[:5000],
                        {"percent": message.get("percent")},
                    )
                )
            elif kind == "usage":
                for key, value in message.items():
                    if key != "type" and isinstance(value, int) and value >= 0:
                        state["usage"][key] = state["usage"].get(key, 0) + value
                on_event(AgentEvent("usage", "", dict(state["usage"])))
            elif kind == "artifact":
                state["artifacts"].append({k: message.get(k) for k in ("name", "path", "kind")})
                on_event(AgentEvent("artifact", str(message.get("name", "")), message))
            elif kind == "result":
                state["result"] = message
                on_event(AgentEvent("result", str(message.get("summary", ""))[:5000], message))
            else:
                on_event(AgentEvent("log", text[:2000]))

        env = base_environment(
            {
                **self.config.env,
                **request.env,
            }
        )
        process = run_process(
            argv,
            cwd=request.workdir,
            env=env,
            timeout=min(request.timeout_seconds, self.config.timeout_seconds),
            stdin_data=json.dumps(payload).encode("utf-8"),
            on_line=on_line,
            should_cancel=should_cancel,
            log_file=request.log_path,
        )
        result = state["result"] or {}
        telemetry = {
            "progress_events": state["progress"],
            "log_lines": state["logs"],
            "output_lines": process.lines,
        }
        if process.timed_out:
            return AgentRunResult(
                "timed_out",
                process.exit_code,
                error="agent timed out",
                usage=state["usage"],
                telemetry=telemetry,
            )
        if process.cancelled:
            return AgentRunResult(
                "cancelled", process.exit_code, error="cancelled", usage=state["usage"], telemetry=telemetry
            )
        declared = result.get("status")
        ok = process.exit_code == 0 and declared in (None, "succeeded")
        cost = result.get("cost_usd")
        return AgentRunResult(
            status="succeeded" if ok else "failed",
            exit_code=process.exit_code,
            summary=str(result.get("summary", ""))[:50_000],
            error=None if ok else (str(result.get("error") or "") or "\n".join(process.tail[-20:]))[:20_000],
            usage=state["usage"],
            cost_usd=float(cost) if isinstance(cost, int | float) and self.config.supports_usage else None,
            artifacts=state["artifacts"],
            telemetry=telemetry,
        )
