"""Claude Code execution adapter.

Uses the documented programmatic interface: ``claude -p`` with
``--output-format stream-json --verbose``. Each stdout line is a JSON message
(``system``/``assistant``/``user``/``result``); human-oriented terminal output is
never scraped. The session id is fixed up front with ``--session-id`` so hooks
fired by that session correlate with the attempt automatically; the
``WORKGRAPH_ATTEMPT_ID``/``WORKGRAPH_TASK_ID`` environment variables are inherited
by hook processes as a second correlation path.
"""

from __future__ import annotations

import json
import subprocess
import uuid
from typing import Any

from workgraph.config.settings import ClaudeCodeAgentConfig
from workgraph.core.errors import ProviderUnavailableError
from workgraph.integrations.agents.base import (
    AgentCapabilities,
    AgentEvent,
    AgentRunRequest,
    AgentRunResult,
    CancelCheck,
    EventSink,
)
from workgraph.integrations.process import base_environment, resolve_executable, run_process

USAGE_FIELDS = ("input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")


def build_argv(config: ClaudeCodeAgentConfig, request: AgentRunRequest, session_id: str) -> list[str]:
    argv = [
        config.executable,
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        config.permission_mode,
    ]
    if request.resume_session_id:
        argv += ["--resume", request.resume_session_id]
    else:
        argv += ["--session-id", session_id]
    model = request.model or config.model
    if model:
        argv += ["--model", model]
    if config.allowed_tools:
        argv += ["--allowedTools", *config.allowed_tools]
    if config.max_turns:
        argv += ["--max-turns", str(config.max_turns)]
    if config.max_budget_usd:
        argv += ["--max-budget-usd", str(config.max_budget_usd)]
    for directory in request.additional_dirs:
        argv += ["--add-dir", str(directory)]
    argv += list(config.extra_args)
    return argv


class StreamState:
    """Incrementally interprets Claude Code ``stream-json`` messages."""

    def __init__(self) -> None:
        self.session_id: str | None = None
        self.model: str | None = None
        self.tool_uses = 0
        self.tool_errors = 0
        self.tools: dict[str, int] = {}
        self.assistant_messages = 0
        self.last_text = ""
        self.result: dict[str, Any] | None = None
        self.subagent_tool_uses = 0

    def handle(self, message: dict[str, Any], on_event: EventSink) -> None:
        kind = message.get("type")
        if kind == "system" and message.get("subtype") == "init":
            self.session_id = message.get("session_id") or self.session_id
            self.model = message.get("model") or self.model
            on_event(
                AgentEvent(
                    "session",
                    "session started",
                    {
                        "session_id": self.session_id,
                        "model": self.model,
                        "tools": len(message.get("tools") or []),
                    },
                )
            )
        elif kind == "assistant":
            self.assistant_messages += 1
            content = (message.get("message") or {}).get("content") or []
            for block in content if isinstance(content, list) else []:
                if block.get("type") == "text" and block.get("text"):
                    self.last_text = block["text"]
                    on_event(AgentEvent("message", block["text"][:2000]))
                elif block.get("type") == "tool_use":
                    self.tool_uses += 1
                    name = str(block.get("name", "unknown"))
                    self.tools[name] = self.tools.get(name, 0) + 1
                    if name in ("Task", "Agent"):
                        self.subagent_tool_uses += 1
                    on_event(AgentEvent("tool_use", name, {"tool_use_id": block.get("id")}))
        elif kind == "user":
            content = (message.get("message") or {}).get("content") or []
            for block in content if isinstance(content, list) else []:
                if block.get("type") == "tool_result" and block.get("is_error"):
                    self.tool_errors += 1
                    on_event(
                        AgentEvent("tool_result", "tool error", {"tool_use_id": block.get("tool_use_id")})
                    )
        elif kind == "result":
            self.result = message
            self.session_id = message.get("session_id") or self.session_id
            on_event(
                AgentEvent(
                    "result",
                    str(message.get("result", ""))[:2000],
                    {"subtype": message.get("subtype"), "is_error": message.get("is_error")},
                )
            )

    def usage(self) -> dict[str, int]:
        if not self.result:
            return {}
        raw = self.result.get("usage") or {}
        usage = {k: int(raw[k]) for k in USAGE_FIELDS if isinstance(raw.get(k), int)}
        if isinstance(self.result.get("num_turns"), int):
            usage["num_turns"] = int(self.result["num_turns"])
        return usage


class ClaudeCodeProvider:
    adapter_type = "claude_code"

    def __init__(self, name: str, config: ClaudeCodeAgentConfig) -> None:
        self.name = name
        self.config = config

    def capabilities(self) -> AgentCapabilities:
        return AgentCapabilities(
            streaming_progress=True,
            usage_reporting=True,
            cost_reporting=True,
            session_resume=True,
            isolated_workspace=True,
            cancellation=True,
            subagents=True,
            lifecycle_hooks=True,
        )

    def check_available(self) -> tuple[bool, str]:
        try:
            exe = resolve_executable(self.config.executable)
        except ProviderUnavailableError as exc:
            return False, exc.message + " (install Claude Code or set agents.claude_code.executable)"
        try:
            out = subprocess.run([exe, "--version"], capture_output=True, text=True, timeout=20, check=False)
        except (OSError, subprocess.TimeoutExpired) as exc:
            return False, f"{exe} --version failed: {exc}"
        return out.returncode == 0, (out.stdout or out.stderr).strip()[:200]

    def config_snapshot(self) -> dict[str, Any]:
        return {
            "adapter": self.adapter_type,
            "executable": self.config.executable,
            "model": self.config.model,
            "permission_mode": self.config.permission_mode,
            "allowed_tools": self.config.allowed_tools,
            "max_turns": self.config.max_turns,
            "max_budget_usd": self.config.max_budget_usd,
            "extra_args": self.config.extra_args,
        }

    def run(
        self, request: AgentRunRequest, on_event: EventSink, should_cancel: CancelCheck
    ) -> AgentRunResult:
        ok, detail = self.check_available()
        if not ok:
            raise ProviderUnavailableError(f"Claude Code is unavailable: {detail}")
        session_id = request.resume_session_id or str(uuid.uuid4())
        argv = build_argv(self.config, request, session_id)
        state = StreamState()
        state.session_id = session_id
        stray: list[str] = []

        def on_line(line: str) -> None:
            text = line.strip()
            if not text:
                return
            try:
                message = json.loads(text)
            except json.JSONDecodeError:
                stray.append(text[:500])
                on_event(AgentEvent("log", text[:2000]))
                return
            if isinstance(message, dict):
                state.handle(message, on_event)

        # Claude Code needs the user's own environment (credentials, config dirs).
        env = base_environment(request.env, inherit_all=True)
        process = run_process(
            argv,
            cwd=request.workdir,
            env=env,
            timeout=min(request.timeout_seconds, self.config.timeout_seconds),
            stdin_data=request.prompt.encode("utf-8"),
            on_line=on_line,
            should_cancel=should_cancel,
            log_file=request.log_path,
        )
        telemetry = {
            "tool_uses": state.tool_uses,
            "tool_errors": state.tool_errors,
            "tools": state.tools,
            "assistant_messages": state.assistant_messages,
            "subagent_launches": state.subagent_tool_uses,
            "unparsed_lines": len(stray),
        }
        result = state.result or {}
        usage = state.usage()
        cost = result.get("total_cost_usd")
        common: dict[str, Any] = {
            "session_id": state.session_id,
            "model": state.model,
            "usage": usage,
            "cost_usd": float(cost) if isinstance(cost, int | float) else None,
            "provider_duration_ms": result.get("duration_ms")
            if isinstance(result.get("duration_ms"), int)
            else None,
            "num_turns": result.get("num_turns") if isinstance(result.get("num_turns"), int) else None,
            "telemetry": telemetry,
        }
        if process.timed_out:
            return AgentRunResult("timed_out", process.exit_code, error="Claude Code timed out", **common)
        if process.cancelled:
            return AgentRunResult("cancelled", process.exit_code, error="cancelled", **common)
        success = process.exit_code == 0 and result.get("subtype") == "success" and not result.get("is_error")
        error = None
        if not success:
            error = str(result.get("result") or result.get("subtype") or "") or "\n".join(process.tail[-20:])
        return AgentRunResult(
            status="succeeded" if success else "failed",
            exit_code=process.exit_code,
            summary=str(result.get("result") or state.last_text)[:50_000],
            error=error[:20_000] if error else None,
            **common,
        )
