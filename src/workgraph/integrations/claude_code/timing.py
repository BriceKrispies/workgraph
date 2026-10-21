"""Where a subagent's wall time went, measured from its transcript.

The transcript's records carry timestamps, so the time is measured rather than inferred:

* a tool call lasts from its assistant ``tool_use`` to the ``tool_result`` with the same id;
* model time is the gap from the last input the model received (a prompt or tool results) to its next
  assistant message;
* waiting is the gap from a turn that ended without a tool call to the next user message that is not a
  tool result, i.e. the agent sat idle until a notification or message resumed it.

Each tool call is classified by ``TOOL_CLASSES``, a data table: a Bash/PowerShell command takes the first
class whose command pattern matches it, and any other tool takes the first class listing its name. Calls
that match no class fall back to ``other shell`` or ``other tool``. A call launched with
``run_in_background`` is recorded under ``<class> (launch)``, since its result returns at once.

Stdlib only: this runs inside the hook entrypoint.
"""

from __future__ import annotations

import re
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any

TIMING_VERSION = 1
SLOWEST_CALLS = 10
COMMAND_CHARS = 160
SYNTHETIC_MODEL = "<synthetic>"
OTHER_SHELL = "other shell"
OTHER_TOOL = "other tool"
POLLING = "polling loop"
SHELL_TOOLS = frozenset({"Bash", "PowerShell"})


@dataclass(frozen=True)
class ToolClass:
    name: str
    tools: frozenset[str] = frozenset()
    commands: tuple[re.Pattern[str], ...] = ()


def _commands(*patterns: str) -> tuple[re.Pattern[str], ...]:
    return tuple(re.compile(pattern, re.IGNORECASE) for pattern in patterns)


# Order matters: the first match wins. Extend the table to split a class further or to add one.
TOOL_CLASSES: tuple[ToolClass, ...] = (
    ToolClass(
        POLLING,
        commands=_commands(
            r"\bsleep\s+\d",
            r"\bStart-Sleep\b",
            r"\b(until|while)\b[^\n]*\bdo\b",
            r"\bwhile\s*\(",
        ),
    ),
    ToolClass("cargo test", commands=_commands(r"\bcargo\s+test\b")),
    ToolClass("cargo build/check", commands=_commands(r"\bcargo\s+(build|check|clippy)\b")),
    ToolClass("dylint gate", commands=_commands(r"dylint")),
    ToolClass("rho build/run", commands=_commands(r"\brho(\.exe)?\s+(build|run|check|describe)\b")),
    ToolClass("program run", commands=_commands(r"\.exe\b")),
    ToolClass("git", commands=_commands(r"^\s*(cd [^&;]*(&&|;)\s*)?git\b")),
    ToolClass(
        "search/read",
        tools=frozenset({"Read", "Grep", "Glob"}),
        commands=_commands(r"\b(grep|rg|sed -n|cat|head|tail|ls|find|wc)\b"),
    ),
    ToolClass("edit", tools=frozenset({"Edit", "Write", "NotebookEdit"})),
    ToolClass("script", commands=_commands(r"\b(python|node|powershell|pwsh)\b")),
    ToolClass("spawn subagent", tools=frozenset({"Agent"})),
    ToolClass("background control", tools=frozenset({"Monitor", "TaskOutput", "TaskStop"})),
)


def classify(name: str, tool_input: dict[str, Any]) -> str:
    """The class of one tool call. Shell commands are matched by text, every other tool by name."""
    if name in SHELL_TOOLS:
        command = tool_input.get("command")
        text = command if isinstance(command, str) else ""
        for tool_class in TOOL_CLASSES:
            if any(pattern.search(text) for pattern in tool_class.commands):
                return tool_class.name
        return OTHER_SHELL
    for tool_class in TOOL_CLASSES:
        if name in tool_class.tools:
            return tool_class.name
    return OTHER_TOOL


def class_names() -> list[str]:
    """Every class the table can produce, in table order, including the fallbacks."""
    return [tool_class.name for tool_class in TOOL_CLASSES] + [OTHER_SHELL, OTHER_TOOL]


def _epoch(value: object) -> float | None:
    if not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _iso(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, tz=UTC).isoformat()


def _describe(name: str, tool_input: dict[str, Any]) -> str:
    """A short, single-line label for a call: its command, or its path, pattern or URL."""
    for key in ("command", "file_path", "path", "pattern", "url", "description"):
        value = tool_input.get(key)
        if isinstance(value, str) and value.strip():
            return " ".join(value.split())[:COMMAND_CHARS]
    return name


def _round(value: float) -> float:
    return round(value, 3)


def breakdown(records: Iterable[Any]) -> dict[str, Any]:
    """The time breakdown of one transcript, from its parsed records (non-dict records are ignored).

    The result is JSON-safe: ``classes`` maps each class to its seconds and call count, ``model_seconds``
    and ``waiting_seconds`` are the non-tool time, ``span_seconds`` runs from the first to the last
    timestamped record, and ``slowest`` lists the longest individual calls.
    """
    events: list[tuple[float, str, dict[str, Any]]] = []
    for record in records:
        if not isinstance(record, dict) or record.get("type") not in ("user", "assistant"):
            continue
        message = record.get("message")
        if not isinstance(message, dict) or message.get("model") == SYNTHETIC_MODEL:
            continue
        at = _epoch(record.get("timestamp"))
        if at is not None:
            events.append((at, record["type"], message))
    events.sort(key=lambda event: event[0])

    pending: dict[str, tuple[float, str, str, str]] = {}  # tool_use id -> (start, class, tool, label)
    class_seconds: dict[str, float] = {}
    class_calls: dict[str, int] = {}
    calls: list[dict[str, Any]] = []
    model_seconds = 0.0
    waiting_seconds = 0.0
    last_input: float | None = None  # when the model last received input
    turn_end: float | None = None  # when the agent last ended a turn without a tool call

    for at, kind, message in events:
        content = message.get("content")
        blocks = [block for block in content if isinstance(block, dict)] if isinstance(content, list) else []
        if kind == "assistant":
            if last_input is not None:
                model_seconds += max(0.0, at - last_input)
                last_input = None
            uses = [block for block in blocks if block.get("type") == "tool_use"]
            for block in uses:
                block_id = block.get("id")
                if not isinstance(block_id, str):
                    continue
                name = block.get("name") if isinstance(block.get("name"), str) else "unknown"
                tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
                cls = classify(name, tool_input)
                if tool_input.get("run_in_background") is True:
                    cls += " (launch)"
                # A message split over several lines repeats its blocks: the first sighting is the start.
                pending.setdefault(block_id, (at, cls, name, _describe(name, tool_input)))
            if not uses:
                turn_end = at
            continue

        results = [block for block in blocks if block.get("type") == "tool_result"]
        for block in results:
            started = pending.pop(block.get("tool_use_id"), None)
            if started is None:
                continue
            start, cls, name, label = started
            duration = max(0.0, at - start)
            class_seconds[cls] = class_seconds.get(cls, 0.0) + duration
            class_calls[cls] = class_calls.get(cls, 0) + 1
            calls.append(
                {"class": cls, "seconds": _round(duration), "tool": name, "command": label, "at": _iso(start)}
            )
        if not results and turn_end is not None:
            # A message after the agent ended its turn resumed it: the agent was waiting on background work.
            waiting_seconds += max(0.0, at - turn_end)
            turn_end = None
        last_input = at

    slowest = sorted(calls, key=lambda call: call["seconds"], reverse=True)[:SLOWEST_CALLS]
    span = events[-1][0] - events[0][0] if events else 0.0
    return {
        "version": TIMING_VERSION,
        "span_seconds": _round(span),
        "tool_seconds": _round(sum(class_seconds.values())),
        "model_seconds": _round(model_seconds),
        "waiting_seconds": _round(waiting_seconds),
        "classes": {
            cls: {"seconds": _round(seconds), "calls": class_calls[cls]}
            for cls, seconds in class_seconds.items()
        },
        "slowest": slowest,
    }
