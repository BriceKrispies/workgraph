"""Measured telemetry from a Claude Code subagent transcript.

Claude Code writes each subagent's transcript as JSONL and a sidecar ``agent-<id>.meta.json``
next to it. This module reads both and reports what actually ran: the model, token usage,
turns, tool calls and errors, plus the ``Task-ID:`` and ``Trial-ID:`` markers in the prompt.

It runs inside the hook entrypoint, so it uses the standard library only.

Dedupe rule: one API message is split over several JSONL lines that share ``message.id``, and
each line carries a usage object whose ``output_tokens`` grows. Usage is taken from the line
with the largest ``output_tokens`` per message id (ties: the last line). Tool uses are collected
from every line and deduplicated by ``tool_use`` id. Synthetic messages (``<synthetic>``) are
ignored entirely.
"""

from __future__ import annotations

import json
import re
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NamedTuple

SYNTHETIC_MODEL = "<synthetic>"
USAGE_FIELDS = ("input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")
TASK_KEY_RE = re.compile(r"^[ \t]*Task-ID:[ \t]*([A-Z][A-Z0-9]{0,9}-\d+)\b", re.MULTILINE)
TRIAL_ID_RE = re.compile(r"^[ \t]*Trial-ID:[ \t]*([0-9A-Za-z][0-9A-Za-z-]{7,})", re.MULTILINE)


@dataclass(frozen=True)
class TranscriptStats:
    models: dict[str, int]  # model -> number of assistant messages
    model: str | None  # the dominant model that ran
    usage: dict[str, int]  # summed over assistant messages, deduplicated per message id
    turns: int  # distinct assistant message ids
    tool_uses: dict[str, int]  # tool name -> calls, deduplicated per tool_use id
    tool_errors: int  # tool_result blocks with is_error true
    prompt_chars: int  # length of the first user prompt text
    task_keys: tuple[str, ...]  # Task-ID markers in the first prompt
    trial_ids: tuple[str, ...]  # Trial-ID markers in the first prompt
    claude_version: str | None
    started_at: str | None  # first record timestamp
    ended_at: str | None  # last record timestamp
    requested_model: str | None = None  # sidecar: the alias the parent asked for
    description: str | None = None  # sidecar
    agent_type: str | None = None  # sidecar

    def telemetry(self) -> dict[str, Any]:
        """JSON-safe dict of every field except ``usage``, which has its own column."""
        return {
            "models": dict(self.models),
            "model": self.model,
            "requested_model": self.requested_model,
            "turns": self.turns,
            "tool_uses": dict(self.tool_uses),
            "tool_errors": self.tool_errors,
            "prompt_chars": self.prompt_chars,
            "task_keys": list(self.task_keys),
            "trial_ids": list(self.trial_ids),
            "claude_version": self.claude_version,
            "started_at": self.started_at,
            "ended_at": self.ended_at,
            "description": self.description,
            "agent_type": self.agent_type,
        }


class _Turn(NamedTuple):
    model: str | None
    output_tokens: int
    usage: dict[str, Any]


def parse_transcript(path: str | Path) -> TranscriptStats | None:
    """Parse a subagent transcript. ``None`` if the file is missing or unreadable."""
    transcript = Path(path)
    try:
        lines = transcript.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    meta = _read_sidecar(transcript)

    turns: dict[str, _Turn] = {}
    tool_uses: dict[str, str] = {}
    tool_errors = 0
    prompt: str | None = None
    claude_version: str | None = None
    started_at: str | None = None
    ended_at: str | None = None
    for number, line in enumerate(lines):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if not isinstance(record, dict):
            continue
        timestamp = record.get("timestamp")
        if isinstance(timestamp, str):
            started_at = started_at or timestamp
            ended_at = timestamp
        version = record.get("version")
        if claude_version is None and isinstance(version, str):
            claude_version = version
        message = record.get("message")
        if not isinstance(message, dict):
            continue
        kind = record.get("type")
        content = message.get("content")
        if kind == "user":
            if prompt is None:
                prompt = _text_of(content)
            tool_errors += _count_tool_errors(content)
        elif kind == "assistant" and message.get("model") != SYNTHETIC_MODEL:
            message_id = message.get("id")
            if not isinstance(message_id, str):
                message_id = f"line-{number}"
            for index, block in enumerate(content if isinstance(content, list) else []):
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    block_id = block.get("id")
                    if not isinstance(block_id, str):
                        block_id = f"{message_id}#{number}.{index}"
                    name = block.get("name")
                    tool_uses[block_id] = name if isinstance(name, str) else "unknown"
            usage = message.get("usage")
            usage = usage if isinstance(usage, dict) else {}
            model = message.get("model")
            output = _count(usage.get("output_tokens"))
            current = turns.get(message_id)
            if current is None or output >= current.output_tokens:
                turns[message_id] = _Turn(model if isinstance(model, str) else None, output, usage)

    totals = dict.fromkeys(USAGE_FIELDS, 0)
    for turn in turns.values():
        for field in USAGE_FIELDS:
            totals[field] += _count(turn.usage.get(field))
    models = Counter(turn.model for turn in turns.values() if turn.model)
    prompt_text = prompt or ""
    return TranscriptStats(
        models=dict(models),
        model=models.most_common(1)[0][0] if models else None,
        usage=totals,
        turns=len(turns),
        tool_uses=dict(Counter(tool_uses.values())),
        tool_errors=tool_errors,
        prompt_chars=len(prompt_text),
        task_keys=tuple(dict.fromkeys(TASK_KEY_RE.findall(prompt_text))),
        trial_ids=tuple(dict.fromkeys(TRIAL_ID_RE.findall(prompt_text))),
        claude_version=claude_version,
        started_at=started_at,
        ended_at=ended_at,
        requested_model=_text_field(meta, "model"),
        description=_text_field(meta, "description"),
        agent_type=_text_field(meta, "agentType"),
    )


@dataclass(frozen=True)
class ToolCall:
    name: str
    input: dict[str, Any]
    tool_use_id: str


def tool_calls(path: str | Path) -> list[ToolCall]:
    """Every tool call a subagent transcript made, one per distinct ``tool_use`` id, in first-seen order.

    Blocks repeated across the lines of one message id collapse to one call (the last occurrence wins, as in
    ``parse_transcript``). Synthetic messages are ignored. A missing or unreadable file gives an empty list.
    """
    try:
        lines = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    calls: dict[str, ToolCall] = {}
    for number, line in enumerate(lines):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if not isinstance(record, dict) or record.get("type") != "assistant":
            continue
        message = record.get("message")
        if not isinstance(message, dict) or message.get("model") == SYNTHETIC_MODEL:
            continue
        message_id = message.get("id")
        if not isinstance(message_id, str):
            message_id = f"line-{number}"
        content = message.get("content")
        for index, block in enumerate(content if isinstance(content, list) else []):
            if not isinstance(block, dict) or block.get("type") != "tool_use":
                continue
            block_id = block.get("id")
            if not isinstance(block_id, str):
                block_id = f"{message_id}#{number}.{index}"
            name = block.get("name")
            tool_input = block.get("input")
            calls[block_id] = ToolCall(
                name=name if isinstance(name, str) else "unknown",
                input=tool_input if isinstance(tool_input, dict) else {},
                tool_use_id=block_id,
            )
    return list(calls.values())


def _read_sidecar(transcript: Path) -> dict[str, Any]:
    sidecar = transcript.with_name(transcript.name.removesuffix(".jsonl") + ".meta.json")
    try:
        data = json.loads(sidecar.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _text_field(data: dict[str, Any], key: str) -> str | None:
    value = data.get(key)
    return value if isinstance(value, str) and value else None


def _count(value: Any) -> int:
    return value if isinstance(value, int) and not isinstance(value, bool) and value > 0 else 0


def _text_of(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            block["text"]
            for block in content
            if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("text"), str)
        )
    return ""


def _count_tool_errors(content: Any) -> int:
    if not isinstance(content, list):
        return 0
    return sum(
        1
        for block in content
        if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("is_error") is True
    )
