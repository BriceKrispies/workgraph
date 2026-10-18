"""Subagent work over time: runs, the outcomes of their linked tasks, and task size.

A subagent run is *linked* when its ``task_id`` names a task in the reported workspace. Outcomes are
read from the task's current status and its acceptance decisions. Size comes from the task's final
change surface when one exists, otherwise from its linked non-merge commits. Runs with no link have
no workspace, so they are counted only when the reported workspace is the default one.
"""

from __future__ import annotations

import statistics
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Literal

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.services.base import Service
from workgraph.experiments.stats import wilson
from workgraph.persistence.orm import (
    Commit,
    CommitTaskLink,
    SubagentRun,
    Task,
    TaskAcceptance,
    TaskChangeSurface,
)
from workgraph.security.permissions import Permission, require

UNKNOWN_MODEL = "unknown"
_PERIODS = ("week", "month")


@dataclass(frozen=True)
class _Run:
    model: str
    at: datetime
    task_id: str | None
    output_tokens: float | None
    turns: float | None
    tool_errors: int


@dataclass(frozen=True)
class _Size:
    lines: int | None
    band: int | None


def _utc(value: datetime) -> datetime:
    return value.replace(tzinfo=UTC) if value.tzinfo is None else value.astimezone(UTC)


def _number(value: object) -> float | None:
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return float(value)


def _dict(value: object) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _bucket(at: datetime, period: str) -> str:
    if period == "week":
        year, week, _ = at.isocalendar()
        return f"{year}-W{week:02d}"
    return at.strftime("%Y-%m")


def _outcome(status: str, decisions: list[str]) -> str:
    if status == "accepted":
        return "accepted"
    if any(decision != "accepted" for decision in decisions):
        return "rejected"
    if status in ("failed", "cancelled"):
        return "failed"
    return "pending"


def _median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def _rate(accepted: int, decided: int) -> tuple[float | None, list[float] | None]:
    if decided == 0:
        return None, None
    lo, hi = wilson(accepted, decided)
    return accepted / decided, [lo, hi]


def _summary(
    runs: list[_Run], tasks: list[str], outcomes: dict[str, str], run_counts: Counter[str]
) -> dict[str, Any]:
    """The measured fields shared by period buckets and model totals."""
    counts = Counter(outcomes[task] for task in tasks)
    accepted, rejected, failed = counts["accepted"], counts["rejected"], counts["failed"]
    decided = accepted + rejected + failed
    rate, interval = _rate(accepted, decided)
    return {
        "runs": len(runs),
        "linked_runs": sum(1 for run in runs if run.task_id is not None),
        "tasks": len(tasks),
        "accepted": accepted,
        "rejected": rejected,
        "failed": failed,
        "pending": counts["pending"],
        "acceptance_rate": rate,
        "wilson": interval,
        "first_pass": sum(1 for task in tasks if outcomes[task] == "accepted" and run_counts[task] == 1),
        "median_output_tokens": _median([r.output_tokens for r in runs if r.output_tokens is not None]),
        "median_turns": _median([r.turns for r in runs if r.turns is not None]),
        "tool_errors": sum(run.tool_errors for run in runs),
    }


def _sizes(session: Session, task_ids: list[str]) -> dict[str, _Size]:
    """Size of each task: its change surface if it has one, else its linked non-merge commits."""
    if not task_ids:
        return {}
    surfaces: dict[str, int] = defaultdict(int)
    for row in session.execute(
        select(TaskChangeSurface.task_id, TaskChangeSurface.insertions, TaskChangeSurface.deletions).where(
            TaskChangeSurface.task_id.in_(task_ids)
        )
    ):
        surfaces[row.task_id] += row.insertions + row.deletions
    commits: dict[str, dict[str, int]] = defaultdict(dict)
    for row in session.execute(
        select(CommitTaskLink.task_id, Commit.id, Commit.insertions, Commit.deletions, Commit.is_merge)
        .join(Commit, Commit.id == CommitTaskLink.commit_id)
        .where(CommitTaskLink.task_id.in_(task_ids))
    ):
        if not row.is_merge:
            commits[row.task_id][row.id] = row.insertions + row.deletions
    sizes: dict[str, _Size] = {}
    for task in task_ids:
        lines: int | None
        if task in surfaces:
            lines = surfaces[task]
        elif commits.get(task):
            lines = sum(commits[task].values())
        else:
            lines = None
        band = lines.bit_length() - 1 if lines is not None and lines >= 1 else None
        sizes[task] = _Size(lines, band)
    return sizes


class SubagentReport(Service):
    def report(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        period: Literal["week", "month"] = "week",
        since: datetime | None = None,
        until: datetime | None = None,
        model: str | None = None,
    ) -> dict[str, Any]:
        """Subagent runs and their linked task outcomes, by period and model. JSON-serialisable."""
        require(ctx, Permission.TASK_READ)
        if period not in _PERIODS:
            raise ValidationFailed(f"unknown period {period!r}", details={"allowed": list(_PERIODS)})
        since_utc = _utc(since) if since else None
        until_utc = _utc(until) if until else None
        default_id = self._default_workspace_id()

        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            is_default = ws_id == default_id
            without_stop = session.scalar(
                select(func.count()).select_from(SubagentRun).where(SubagentRun.stopped_at.is_(None))
            )
            stopped = list(session.scalars(select(SubagentRun).where(SubagentRun.stopped_at.is_not(None))))
            task_ids = sorted({run.task_id for run in stopped if run.task_id is not None})
            owners: dict[str, tuple[str, str]] = {
                row.id: (row.workspace_id, row.status)
                for row in session.execute(
                    select(Task.id, Task.workspace_id, Task.status).where(Task.id.in_(task_ids))
                )
            }
            ws_task_ids = sorted(task for task, (owner, _) in owners.items() if owner == ws_id)
            decisions: dict[str, list[str]] = defaultdict(list)
            if ws_task_ids:
                for row in session.execute(
                    select(TaskAcceptance.task_id, TaskAcceptance.decision).where(
                        TaskAcceptance.task_id.in_(ws_task_ids)
                    )
                ):
                    decisions[row.task_id].append(row.decision)
            sizes = _sizes(session, ws_task_ids)

        outcomes = {task: _outcome(owners[task][1], decisions.get(task, [])) for task in ws_task_ids}
        runs, unlinked, outside, trials = self._runs(
            stopped, owners, ws_id, is_default, since_utc, until_utc, model
        )

        models = sorted({run.model for run in runs})
        per_model = {name: [run for run in runs if run.model == name] for name in models}
        per_model_tasks = {
            name: sorted({run.task_id for run in group if run.task_id is not None})
            for name, group in per_model.items()
        }
        run_counts: dict[str, Counter[str]] = {
            name: Counter(run.task_id for run in group if run.task_id is not None)
            for name, group in per_model.items()
        }

        notes: list[str] = []
        if without_stop:
            notes.append(f"{without_stop} subagent runs have no stopped_at and are excluded from the report")
        if unlinked:
            notes.append(
                f"{unlinked} runs have no Task-ID link; start a subagent prompt with `Task-ID: <KEY>` to link it"
            )
            if not is_default:
                notes.append("unlinked runs are counted only when the default workspace is reported")
        if outside:
            notes.append(f"{outside} runs are linked to tasks in other workspaces and are excluded")
        if trials:
            notes.append(
                f"{trials} runs are experiment trials (their prompt carries a Trial-ID) and are excluded; "
                "see `workgraph experiment report`"
            )
        unknown = sum(1 for name in models for task in per_model_tasks[name] if sizes[task].lines is None)
        if unknown:
            notes.append(
                f"{unknown} linked tasks have no change surface or linked commits, so their size is unknown"
            )
        notes.append(
            "A task's outcome is its current status: accepted, rejected (a non-accepted acceptance decision), "
            "failed or cancelled; anything else is pending"
        )
        return {
            "period": period,
            "window": {
                "since": since_utc.isoformat() if since_utc else None,
                "until": until_utc.isoformat() if until_utc else None,
            },
            "model": model,
            "by_period": self._by_period(per_model, per_model_tasks, outcomes, run_counts, period),
            "by_size": self._by_size(per_model, per_model_tasks, outcomes, sizes),
            "totals": [
                {"model": name, **_summary(group, per_model_tasks[name], outcomes, run_counts[name])}
                for name, group in per_model.items()
            ],
            "unlinked_runs": unlinked,
            "notes": notes,
        }

    # ------------------------------------------------------------------ helpers

    def _default_workspace_id(self) -> str | None:
        try:
            return self.app.default_workspace_id()
        except NotFoundError:
            return None

    @staticmethod
    def _runs(
        stopped: list[SubagentRun],
        owners: dict[str, tuple[str, str]],
        ws_id: str,
        is_default: bool,
        since: datetime | None,
        until: datetime | None,
        model: str | None,
    ) -> tuple[list[_Run], int, int, int]:
        """The in-window runs of the report, plus the counts of unlinked, out-of-workspace and trial runs."""
        runs: list[_Run] = []
        unlinked = 0
        outside = 0
        trials = 0
        for row in stopped:
            at = _utc(row.stopped_at) if row.stopped_at is not None else None
            if at is None or (since is not None and at < since) or (until is not None and at >= until):
                continue
            name = row.model or row.requested_model or UNKNOWN_MODEL
            if model is not None and name != model:
                continue
            telemetry = _dict(row.telemetry)
            # Experiment trials have their own report and never reach acceptance, so they would read as pending work.
            if telemetry.get("trial_ids"):
                trials += 1
                continue
            owner = owners.get(row.task_id) if row.task_id is not None else None
            if owner is not None and owner[0] != ws_id:
                outside += 1
                continue
            if owner is None:
                unlinked += 1
                if not is_default:
                    continue
            usage = _dict(row.usage)
            runs.append(
                _Run(
                    model=name,
                    at=at,
                    task_id=row.task_id if owner is not None else None,
                    output_tokens=_number(usage.get("output_tokens")),
                    turns=_number(telemetry.get("turns")),
                    tool_errors=int(_number(telemetry.get("tool_errors")) or 0),
                )
            )
        return runs, unlinked, outside, trials

    @staticmethod
    def _by_period(
        per_model: dict[str, list[_Run]],
        per_model_tasks: dict[str, list[str]],
        outcomes: dict[str, str],
        run_counts: dict[str, Counter[str]],
        period: str,
    ) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for name, group in per_model.items():
            buckets: dict[str, list[_Run]] = defaultdict(list)
            for run in group:
                buckets[_bucket(run.at, period)].append(run)
            # A task belongs to the bucket of its latest run of this model.
            latest: dict[str, _Run] = {}
            for run in group:
                if run.task_id is None:
                    continue
                current = latest.get(run.task_id)
                if current is None or run.at > current.at:
                    latest[run.task_id] = run
            task_buckets: dict[str, list[str]] = defaultdict(list)
            for task, run in latest.items():
                task_buckets[_bucket(run.at, period)].append(task)
            for key in sorted(buckets):
                tasks = sorted(task_buckets.get(key, []))
                rows.append(
                    {
                        "period": key,
                        "model": name,
                        **_summary(buckets[key], tasks, outcomes, run_counts[name]),
                    }
                )
        return sorted(rows, key=lambda row: (row["period"], row["model"]))

    @staticmethod
    def _by_size(
        per_model: dict[str, list[_Run]],
        per_model_tasks: dict[str, list[str]],
        outcomes: dict[str, str],
        sizes: dict[str, _Size],
    ) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for name in per_model:
            groups: dict[int | None, list[str]] = defaultdict(list)
            for task in per_model_tasks[name]:
                groups[sizes[task].band].append(task)
            for band in sorted(groups, key=lambda b: (b is None, b or 0)):
                tasks = groups[band]
                counts = Counter(outcomes[task] for task in tasks)
                accepted, rejected, failed = counts["accepted"], counts["rejected"], counts["failed"]
                decided = accepted + rejected + failed
                rate, interval = _rate(accepted, decided)
                lines: list[float] = []
                if band is not None:
                    for task in tasks:
                        size_lines = sizes[task].lines
                        if size_lines is not None:
                            lines.append(float(size_lines))
                rows.append(
                    {
                        "model": name,
                        "band": band,
                        "tasks": len(tasks),
                        "accepted": accepted,
                        "decided": decided,
                        "acceptance_rate": rate,
                        "wilson": interval,
                        "median_lines": _median(lines),
                    }
                )
        return rows
