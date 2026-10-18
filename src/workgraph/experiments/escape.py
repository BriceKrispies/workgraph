"""Tool calls that reach outside a trial's clone, found by the path text in their inputs.

A trial's solution lives in the repository's own checkout at HEAD, so a subagent that reads that checkout has
left its clone. Matching is on text: every string in a tool call's input is normalised (lowercase, forward
slashes, MSYS ``/c/`` drive prefixes turned into ``c:/``, repeated slashes collapsed) and checked for a
forbidden root followed by the end of the string, a slash, a quote, whitespace, or ``;``, ``&`` or ``|``.
"""

from __future__ import annotations

import re
from collections.abc import Iterator, Sequence
from dataclasses import dataclass
from typing import Any

from workgraph.integrations.claude_code.transcript import ToolCall

EXCERPT_CHARS = 300
_REPEATED_SLASH = re.compile(r"/{2,}")
# A single-letter segment at the start of a path, not preceded by a word character, colon or slash: "/c/dev".
_MSYS_DRIVE = re.compile(r"(?<![\w:/])/([a-z])/")
# A root ends where a path ends: at a separator, a quote, whitespace, or a shell operator.
_ROOT_END = r"(?=\Z|[/\"'\s;&|])"


def normalize_path_text(text: str) -> str:
    lowered = text.lower().replace("\\", "/")
    collapsed = _REPEATED_SLASH.sub("/", lowered)
    return _MSYS_DRIVE.sub(r"\1:/", collapsed)


@dataclass(frozen=True)
class Escape:
    tool: str
    tool_use_id: str
    root: str
    excerpt: str


def find_escapes(calls: Sequence[ToolCall], forbidden_roots: Sequence[str]) -> list[Escape]:
    """One Escape per (call, root) whose input mentions the root. The excerpt is the offending input string."""
    roots: list[tuple[str, str]] = []
    for root in forbidden_roots:
        normalized = normalize_path_text(root).rstrip("/")
        if normalized and root not in [original for original, _ in roots]:
            roots.append((root, normalized))

    escapes: list[Escape] = []
    for call in calls:
        values = [(raw, normalize_path_text(raw)) for raw in _strings(call.input)]
        for original, normalized in roots:
            pattern = re.compile(re.escape(normalized) + _ROOT_END)
            hit = next((raw for raw, norm in values if pattern.search(norm)), None)
            if hit is not None:
                escapes.append(
                    Escape(
                        tool=call.name,
                        tool_use_id=call.tool_use_id,
                        root=original,
                        excerpt=hit[:EXCERPT_CHARS],
                    )
                )
    return escapes


def _strings(value: Any) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from _strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from _strings(item)
