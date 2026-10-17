"""Commit-to-task attribution rules.

Sources, in decreasing order of authority:

1. ``explicit``            - a person or agent linked the commit (confidence 1.0)
2. ``execution_context``   - created inside a Workgraph-managed attempt (0.95)
3. ``worktree``            - reachable from a managed worktree branch (0.9)
4. ``trailer``             - a ``Task-ID:`` commit trailer (0.95)
5. ``message_inference``   - a task key mentioned in the message (0.5, labelled inferred)

Each link records its source, confidence and the evidence used, so every derived
number can be traced back to why a commit was attributed to a task.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any

TRAILER_KEYS = {"task-id", "workgraph-task", "wg-task"}


@dataclass
class Candidate:
    task_ref: str
    source: str
    confidence: float
    evidence: dict[str, Any] = field(default_factory=dict)


def trailer_candidates(trailers: list[dict[str, str]]) -> list[Candidate]:
    found: list[Candidate] = []
    for trailer in trailers:
        if trailer["key"].lower() in TRAILER_KEYS:
            for value in re.split(r"[,\s]+", trailer["value"]):
                if value:
                    found.append(
                        Candidate(
                            value, "trailer", 0.95, {"trailer": f"{trailer['key']}: {trailer['value']}"}
                        )
                    )
    return found


def message_candidates(message: str, key_prefix: str, trailers: list[dict[str, str]]) -> list[Candidate]:
    """Task keys mentioned in prose. Ambiguous (several keys) lowers confidence further."""
    trailer_lines = {f"{t['key']}: {t['value']}" for t in trailers}
    prose = "\n".join(line for line in message.splitlines() if line.strip() not in trailer_lines)
    pattern = re.compile(rf"(?<![A-Za-z0-9]){re.escape(key_prefix)}-(\d+)(?![0-9])", re.IGNORECASE)
    keys = list(dict.fromkeys(f"{key_prefix}-{m.group(1)}" for m in pattern.finditer(prose)))
    if not keys:
        return []
    ambiguous = len(keys) > 1
    confidence = 0.35 if ambiguous else 0.5
    return [
        Candidate(
            key,
            "message_inference",
            confidence,
            {
                "label": "inferred from commit message",
                "matched": key,
                "ambiguous": ambiguous,
                "all_matches": keys,
            },
        )
        for key in keys
    ]
