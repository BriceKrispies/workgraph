"""Where subagent wall time goes: the measured time breakdown of each run, aggregated.

Each stopped run whose transcript was read carries ``telemetry["time"]``, written by
``integrations/claude_code/timing.py``: seconds per tool class, model time, waiting on background work and
the slowest individual calls. Runs are selected as the subagent report selects them: stopped inside the
window, linked to a task in the reported workspace (or unlinked, when that workspace is the default one),
and not experiment trials.

Concurrency is machine-wide. It counts every stopped subagent run, in any workspace, whose interval
contains an instant, so a slow call that ran while many agents were live shows the saturation it ran in.
"""

from __future__ import annotations

import bisect
import statistics
from collections import defaultdict
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Literal

from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.services.base import Service
from workgraph.integrations.claude_code.timing import POLLING, class_names
from workgraph.persistence.orm import SubagentRun, Task
from workgraph.security.permissions import Permission, require

GROUPS = ("class", "model", "task", "description")
MODEL_KIND = "model"
WAITING_KIND = "waiting"
WAITING_CLASS = "waiting on background"
MODEL_CLASS = "model"
UNLINKED = "(unlinked)"
UNKNOWN = "unknown"
TOP_CLASSES = 3


@dataclass(frozen=True)
class _Timed:
    run_id: str
    task_key: str | None
    description: str
    model: str
    start: float | None
    stop: float
    time: dict[str, Any]


def _utc(value: datetime) -> datetime:
    return value.replace(tzinfo=UTC) if value.tzinfo is None else value.astimezone(UTC)


def _epoch(value: object) -> float | None:
    if isinstance(value, datetime):
        return _utc(value).timestamp()
    if not isinstance(value, str):
        return None
    try:
        return _utc(datetime.fromisoformat(value.replace("Z", "+00:00"))).timestamp()
    except ValueError:
        return None


def _number(value: object) -> float:
    return float(value) if isinstance(value, int | float) and not isinstance(value, bool) else 0.0


def _dict(value: object) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _share(part: float, whole: float) -> float | None:
    return part / whole if whole > 0 else None


def _accounted(time: dict[str, Any]) -> float:
    return (
        _number(time.get("tool_seconds"))
        + _number(time.get("model_seconds"))
        + _number(time.get("waiting_seconds"))
    )


def _class_seconds(time: dict[str, Any]) -> dict[str, float]:
    return {name: _number(_dict(value).get("seconds")) for name, value in _dict(time.get("classes")).items()}


class AgentTimeReport(Service):
    def breakdown(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        since: datetime | None = None,
        until: datetime | None = None,
        by: Literal["class", "model", "task", "description"] = "class",
        model: str | None = None,
    ) -> dict[str, Any]:
        """Share of subagent wall time by tool class, or per group (model, task, description). JSON-serialisable."""
        if by not in GROUPS:
            raise ValidationFailed(f"unknown grouping {by!r}", details={"allowed": list(GROUPS)})
        runs, notes, _ = self._load(ctx, workspace, since, until, model)
        result: dict[str, Any] = {
            "by": by,
            "window": _window(since, until),
            "model": model,
            "totals": _totals(runs),
            "notes": notes,
        }
        if by == "class":
            result["rows"] = _class_rows(runs)
        else:
            result["rows"] = _group_rows(runs, by)
        return result

    def bottlenecks(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        since: datetime | None = None,
        until: datetime | None = None,
        top: int = 15,
    ) -> dict[str, Any]:
        """Where the time went, the slowest calls with the concurrency they ran under, and per-task wall time."""
        runs, notes, intervals = self._load(ctx, workspace, since, until, None)
        starts = sorted(start for start, _ in intervals)
        stops = sorted(stop for _, stop in intervals)

        def active(at: float) -> int:
            # Runs that started by ``at`` and had not stopped before it.
            return bisect.bisect_right(starts, at) - bisect.bisect_left(stops, at)

        slowest: list[dict[str, Any]] = []
        for run in runs:
            for call in run.time.get("slowest") or []:
                if not isinstance(call, dict):
                    continue
                at = _epoch(call.get("at"))
                slowest.append(
                    {
                        "class": call.get("class") or UNKNOWN,
                        "seconds": _number(call.get("seconds")),
                        "tool": call.get("tool"),
                        "command": call.get("command"),
                        "at": call.get("at"),
                        "description": run.description,
                        "model": run.model,
                        "task": run.task_key,
                        "concurrent_runs": active(at) if at is not None else None,
                    }
                )
        slowest.sort(key=lambda call: call["seconds"], reverse=True)
        slowest = slowest[:top]
        concurrency = [call["concurrent_runs"] for call in slowest if call["concurrent_runs"] is not None]
        peak = max((active(start) for start, _ in intervals if _in_window(start, since, until)), default=0)

        totals = _totals(runs)
        return {
            "window": _window(since, until),
            "totals": totals,
            "classes": _class_rows(runs),
            "slowest_calls": slowest,
            "concurrency": {
                "peak_during_window": peak,
                "median_at_slowest_calls": statistics.median(concurrency) if concurrency else None,
                "scope": "stopped subagent runs in every workspace on this machine",
            },
            "by_task": _group_rows(runs, "task")[:top],
            "notes": notes,
        }

    # ------------------------------------------------------------------ helpers

    def _load(
        self,
        ctx: RequestContext,
        workspace: str | None,
        since: datetime | None,
        until: datetime | None,
        model: str | None,
    ) -> tuple[list[_Timed], list[str], list[tuple[float, float]]]:
        """The in-scope timed runs, the notes, and the (start, stop) interval of every stopped run."""
        require(ctx, Permission.TASK_READ)
        since_utc = _utc(since) if since else None
        until_utc = _utc(until) if until else None
        since_s = since_utc.timestamp() if since_utc else None
        until_s = until_utc.timestamp() if until_utc else None
        default_id = self._default_workspace_id()

        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stopped = list(session.scalars(select(SubagentRun).where(SubagentRun.stopped_at.is_not(None))))
            task_ids = sorted({run.task_id for run in stopped if run.task_id is not None})
            owners: dict[str, tuple[str, str]] = {}
            if task_ids:
                owners = {
                    row.id: (row.workspace_id, row.key)
                    for row in session.execute(
                        select(Task.id, Task.workspace_id, Task.key).where(Task.id.in_(task_ids))
                    )
                }

        runs: list[_Timed] = []
        intervals: list[tuple[float, float]] = []
        notes: list[str] = []
        untimed = unlinked = outside = trials = 0
        for row in stopped:
            telemetry = _dict(row.telemetry)
            stop = _epoch(row.stopped_at)
            if stop is None:
                continue
            start = _epoch(row.started_at) or _epoch(telemetry.get("started_at"))
            if start is not None:
                intervals.append((start, max(start, stop)))
            if (since_s is not None and stop < since_s) or (until_s is not None and stop >= until_s):
                continue
            name = row.model or row.requested_model or UNKNOWN
            if model is not None and name != model:
                continue
            if telemetry.get("trial_ids"):
                trials += 1
                continue
            owner = owners.get(row.task_id) if row.task_id is not None else None
            if owner is not None and owner[0] != ws_id:
                outside += 1
                continue
            if owner is None:
                unlinked += 1
                if default_id != ws_id:
                    continue
            time = _dict(telemetry.get("time"))
            if not time:
                untimed += 1
                continue
            runs.append(
                _Timed(
                    run_id=row.id,
                    task_key=owner[1] if owner is not None else None,
                    description=telemetry.get("description") or row.agent_type or UNKNOWN,
                    model=name,
                    start=start,
                    stop=stop,
                    time=time,
                )
            )

        if untimed:
            notes.append(
                f"{untimed} runs have no time breakdown; run `workgraph claude backfill-time` to compute it"
            )
        if unlinked and default_id != ws_id:
            notes.append("unlinked runs are counted only when the default workspace is reported")
        if outside:
            notes.append(f"{outside} runs are linked to tasks in other workspaces and are excluded")
        if trials:
            notes.append(f"{trials} runs are experiment trials and are excluded")
        notes.append(
            "Waiting is the time an agent sat idle after ending a turn until a notification resumed it; model "
            "time is the gap before each assistant message. Shares are of accounted time (tool + model + waiting)."
        )
        return runs, notes, intervals

    def _default_workspace_id(self) -> str | None:
        try:
            return self.app.default_workspace_id()
        except NotFoundError:
            return None


def _in_window(at: float, since: datetime | None, until: datetime | None) -> bool:
    if since is not None and at < _utc(since).timestamp():
        return False
    return not (until is not None and at >= _utc(until).timestamp())


def _window(since: datetime | None, until: datetime | None) -> dict[str, str | None]:
    return {
        "since": _utc(since).isoformat() if since else None,
        "until": _utc(until).isoformat() if until else None,
    }


def _totals(runs: list[_Timed]) -> dict[str, Any]:
    tool = sum(_number(run.time.get("tool_seconds")) for run in runs)
    model = sum(_number(run.time.get("model_seconds")) for run in runs)
    waiting = sum(_number(run.time.get("waiting_seconds")) for run in runs)
    accounted = tool + model + waiting
    polling = sum(_class_seconds(run.time).get(POLLING, 0.0) for run in runs)
    return {
        "runs": len(runs),
        "span_seconds": round(sum(_number(run.time.get("span_seconds")) for run in runs), 3),
        "accounted_seconds": round(accounted, 3),
        "tool_seconds": round(tool, 3),
        "model_seconds": round(model, 3),
        "waiting_seconds": round(waiting, 3),
        "polling_seconds": round(polling, 3),
        "model_share": _share(model, accounted),
        "waiting_share": _share(waiting, accounted),
        "polling_share": _share(polling, accounted),
    }


def _class_rows(runs: list[_Timed]) -> list[dict[str, Any]]:
    """Each tool class, plus model time and waiting, as shares of accounted time (they sum to 1)."""
    seconds: dict[str, float] = defaultdict(float)
    calls: dict[str, int] = defaultdict(int)
    for run in runs:
        for name, value in _dict(run.time.get("classes")).items():
            seconds[name] += _number(_dict(value).get("seconds"))
            calls[name] += int(_number(_dict(value).get("calls")))
        seconds[MODEL_CLASS] += _number(run.time.get("model_seconds"))
        seconds[WAITING_CLASS] += _number(run.time.get("waiting_seconds"))
    accounted = sum(seconds.values())
    order = {name: index for index, name in enumerate(class_names())}
    rows = [
        {
            "class": name,
            "kind": MODEL_KIND if name == MODEL_CLASS else WAITING_KIND if name == WAITING_CLASS else "tool",
            "seconds": round(value, 3),
            "calls": calls.get(name, 0),
            "share": _share(value, accounted),
        }
        for name, value in seconds.items()
        if value > 0 or calls.get(name)
    ]
    return sorted(rows, key=lambda row: (-row["seconds"], order.get(row["class"], len(order))))


def _group_rows(runs: list[_Timed], by: str) -> list[dict[str, Any]]:
    groups: dict[str, list[_Timed]] = defaultdict(list)
    for run in runs:
        if by == "task":
            key = run.task_key or UNLINKED
        elif by == "model":
            key = run.model
        else:
            key = run.description
        groups[key].append(run)
    rows: list[dict[str, Any]] = []
    for key, group in groups.items():
        classes: dict[str, float] = defaultdict(float)
        for run in group:
            for name, value in _class_seconds(run.time).items():
                classes[name] += value
        total = _totals(group)
        top = sorted(classes.items(), key=lambda item: -item[1])
        rows.append(
            {
                "key": key,
                "runs": total["runs"],
                "span_seconds": total["span_seconds"],
                "accounted_seconds": total["accounted_seconds"],
                "model_seconds": total["model_seconds"],
                "waiting_seconds": total["waiting_seconds"],
                "polling_share": total["polling_share"],
                "top_classes": [
                    {"class": name, "seconds": round(value, 3)} for name, value in top[:TOP_CLASSES]
                ],
                "classes": {name: round(value, 3) for name, value in top},
            }
        )
    rows.sort(key=lambda row: (-row["span_seconds"], row["key"]))
    return rows
