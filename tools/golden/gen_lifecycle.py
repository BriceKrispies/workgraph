"""Golden vectors for the task and attempt lifecycle policy (GV-01).

Run from the repository root with `uv run python tools/golden/gen_lifecycle.py`. Writes
`golden/lifecycle/*.json`. The output is deterministic: sorted keys, LF line endings, UTF-8.

Files written:
- task_transitions.json: every (from, to) pair of task statuses, each combined with the
  `privileged` and `tracker` flags, and the Python outcome (ok, or the error code, message and details).
- attempt_transitions.json: every (from, to) pair of attempt statuses and its outcome.
- constants.json: the transition table, the restricted targets, the tracker close targets and
  the statuses that require satisfied dependencies, as the Python module defines them.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from workgraph.core.errors import InvalidTransitionError
from workgraph.core.models.enums import AttemptStatus, TaskStatus
from workgraph.core.policies import lifecycle

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "lifecycle"


def _plain(value: Any) -> Any:
    """Convert StrEnum members, sets and frozensets to plain JSON-compatible values."""
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, (set, frozenset)):
        return sorted(str(v) for v in value)
    if isinstance(value, str):
        return str(value)
    return value


def _task_outcome(current: TaskStatus, target: TaskStatus, privileged: bool, tracker: bool) -> dict[str, Any]:
    try:
        lifecycle.check_task_transition(current, target, privileged=privileged, tracker=tracker)
    except InvalidTransitionError as exc:
        return {
            "result": "error",
            "code": exc.code,
            "message": exc.message,
            "details": _plain(exc.details),
        }
    return {"result": "ok"}


def _attempt_outcome(current: AttemptStatus, target: AttemptStatus) -> dict[str, Any]:
    try:
        lifecycle.check_attempt_transition(current, target)
    except InvalidTransitionError as exc:
        return {
            "result": "error",
            "code": exc.code,
            "message": exc.message,
            "details": _plain(exc.details),
        }
    return {"result": "ok"}


def task_vectors() -> list[dict[str, Any]]:
    vectors: list[dict[str, Any]] = []
    for current in TaskStatus:
        for target in TaskStatus:
            for privileged in (False, True):
                for tracker in (False, True):
                    case = {
                        "from": str(current),
                        "to": str(target),
                        "privileged": privileged,
                        "tracker": tracker,
                    }
                    case.update(_task_outcome(current, target, privileged, tracker))
                    vectors.append(case)
    return vectors


def attempt_vectors() -> list[dict[str, Any]]:
    vectors: list[dict[str, Any]] = []
    for current in AttemptStatus:
        for target in AttemptStatus:
            case = {"from": str(current), "to": str(target)}
            case.update(_attempt_outcome(current, target))
            vectors.append(case)
    return vectors


def constants() -> dict[str, Any]:
    return {
        "task_transitions": {
            str(k): sorted(str(v) for v in vs) for k, vs in lifecycle.TASK_TRANSITIONS.items()
        },
        "attempt_transitions": {
            str(k): sorted(str(v) for v in vs) for k, vs in lifecycle.ATTEMPT_TRANSITIONS.items()
        },
        "restricted_targets": sorted(str(s) for s in lifecycle.RESTRICTED_TARGETS),
        "tracker_close_targets": sorted(str(s) for s in lifecycle.TRACKER_CLOSE_TARGETS),
        "requires_dependencies": sorted(str(s) for s in lifecycle.REQUIRES_DEPENDENCIES),
    }


def _write(name: str, payload: Any) -> None:
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT_DIR / name).write_bytes(text.encode("utf-8"))


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    _write("task_transitions.json", {"vectors": task_vectors()})
    _write("attempt_transitions.json", {"vectors": attempt_vectors()})
    _write("constants.json", constants())


if __name__ == "__main__":
    main()
