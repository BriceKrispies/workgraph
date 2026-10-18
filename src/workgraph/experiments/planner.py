"""Trial planner: choose the next trial of an experiment, and report what its trials measured.

``next`` follows the protocol. The primary arm (``arms[0]``) walks the band staircase, refines around
its threshold and repeats some of its tasks; the control arm then runs tasks around the threshold.
Each choice is a pure function of the protocol, the trial history and the seed, so the same history
plans the same trials. ``report`` summarises the same history per arm: pass rates by band, a size fit
with bootstrap intervals, token usage and failure causes.
"""

from __future__ import annotations

import math
import random
import statistics
from collections import Counter
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from typing import Any

from workgraph.core.context import RequestContext
from workgraph.core.models.enums import TrialStatus
from workgraph.core.models.schemas import TaskFilter, TrialOut
from workgraph.core.services.base import Service
from workgraph.experiments.pool import POOL_TAG
from workgraph.experiments.staircase import StaircaseConfig, Step, next_step
from workgraph.experiments.stats import (
    bootstrap_interval,
    fit_logistic,
    size_at_probability,
    wilson,
)
from workgraph.persistence.orm import SubagentRun
from workgraph.security.permissions import Permission, require

_SEED_STRIDE = 1_000_003
_BOOTSTRAP_RESAMPLES = 1000
_MIN_FIT_TRIALS = 4
_USAGE_KEYS = ("output_tokens", "cache_read_input_tokens")
_TELEMETRY_KEYS = ("turns", "tool_errors")


@dataclass(frozen=True)
class _Task:
    key: str
    band: int
    replay: dict[str, Any]


@dataclass(frozen=True)
class _FitRow:
    task_id: str
    x: float
    y: int


# ---------------------------------------------------------------------- pure helpers


def _number(value: object) -> float | None:
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return float(value)


def _dict(value: object) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _text(value: object) -> str | None:
    return None if value is None else str(value)


def _replay_features(trial: TrialOut) -> dict[str, Any] | None:
    """The replay features a trial was planned with (``add_trial`` stores them under ``predicted``)."""
    for source in (trial.features.get("predicted"), trial.features):
        replay = _dict(source).get("replay")
        if isinstance(replay, dict):
            return replay
    return None


def _requested_band(trial: TrialOut) -> int | None:
    """The band the staircase asked for. A substituted task keeps the requested band for visit grouping."""
    requested = _dict(trial.features.get("predicted")).get("band_requested")
    if isinstance(requested, int) and not isinstance(requested, bool):
        return requested
    return trial.band


def _primary(protocol: dict[str, Any]) -> str:
    return str(protocol["arms"][0]["name"])


def _count(protocol: dict[str, Any], section: str) -> int:
    value = _dict(protocol.get(section)).get("trials")
    return value if isinstance(value, int) and not isinstance(value, bool) else 0


def _step(protocol: dict[str, Any], trials: Sequence[TrialOut]) -> Step:
    """The staircase state, from the primary arm's graded trials only."""
    primary = _primary(protocol)
    histories: dict[str, list[tuple[int, bool]]] = {"staircase": [], "refine": []}
    for trial in trials:
        if trial.arm != primary or trial.status != TrialStatus.GRADED or trial.phase not in histories:
            continue
        band = _requested_band(trial)
        if band is not None:
            histories[trial.phase].append((band, trial.outcome == "pass"))
    return next_step(StaircaseConfig.from_protocol(protocol), histories["staircase"], histories["refine"])


def _step_dict(step: Step) -> dict[str, Any]:
    return {
        "phase": step.phase,
        "band": step.band,
        "reversals": step.reversals,
        "threshold": step.threshold,
        "visits": [list(visit) for visit in step.visits],
    }


def _result(done: bool, phase: str | None, trial: TrialOut | None, reason: str, step: Step) -> dict[str, Any]:
    return {"done": done, "phase": phase, "trial": trial, "reason": reason, "step": _step_dict(step)}


def _graded_tasks(
    valid: Sequence[TrialOut], primary: str, phases: Sequence[str] | None = None
) -> list[_Task]:
    """Primary-arm graded tasks, one per key, optionally limited to some phases, sorted by key."""
    found: dict[str, _Task] = {}
    for trial in valid:
        if trial.arm != primary or trial.status != TrialStatus.GRADED or trial.band is None:
            continue
        if phases is not None and trial.phase not in phases:
            continue
        replay = _replay_features(trial)
        if replay is not None:
            found.setdefault(trial.task_key, _Task(trial.task_key, trial.band, replay))
    return [found[key] for key in sorted(found)]


def _pick(seed: int, next_seq: int, tasks: Sequence[_Task]) -> _Task:
    """Deterministic choice: the same seed, history position and candidates give the same task."""
    rng = random.Random(seed * _SEED_STRIDE + next_seq)
    return rng.choice(sorted(tasks, key=lambda task: task.key))


def _nearest_unused(pool: Sequence[_Task], used: set[str], band: int) -> tuple[int, list[_Task]] | None:
    """The unused tasks of ``band``, or of the nearest band that has one (ties go to the lower band)."""
    by_band: dict[int, list[_Task]] = {}
    for task in pool:
        if task.key not in used:
            by_band.setdefault(task.band, []).append(task)
    if not by_band:
        return None
    chosen = min(by_band, key=lambda b: (abs(b - band), b))
    return chosen, by_band[chosen]


def _summary(values: Iterable[float]) -> dict[str, float | None]:
    collected = list(values)
    if not collected:
        return {"median": None, "mean": None}
    return {"median": float(statistics.median(collected)), "mean": float(statistics.fmean(collected))}


def _sorted_counts(values: Iterable[str]) -> dict[str, int]:
    return dict(sorted(Counter(values).items()))


def _band_rows(graded: Sequence[TrialOut]) -> list[dict[str, Any]]:
    by_band: dict[int, list[bool]] = {}
    for trial in graded:
        if trial.band is not None:
            by_band.setdefault(trial.band, []).append(trial.outcome == "pass")
    rows: list[dict[str, Any]] = []
    for band in sorted(by_band):
        outcomes = by_band[band]
        passes = sum(outcomes)
        lo, hi = wilson(passes, len(outcomes))
        rows.append(
            {
                "band": band,
                "n": len(outcomes),
                "passes": passes,
                "rate": passes / len(outcomes),
                "wilson": [lo, hi],
            }
        )
    return rows


def _lines(x: float | None) -> float | None:
    return None if x is None else 2.0**x


def _threshold_statistic(p: float) -> Callable[[Sequence[_FitRow]], float | None]:
    """Refit the size model on a bootstrap sample and return its size at probability ``p``."""

    def statistic(sample: Sequence[_FitRow]) -> float | None:
        refit = fit_logistic([[row.x] for row in sample], [row.y for row in sample])
        if not refit.converged or refit.coef[1] >= 0:
            return None
        return size_at_probability(refit, p)

    return statistic


def _lines_interval(rows: Sequence[_FitRow], p: float, seed: int) -> list[float] | None:
    interval = bootstrap_interval(
        rows,
        _threshold_statistic(p),
        cluster=lambda row: row.task_id,
        n=_BOOTSTRAP_RESAMPLES,
        seed=seed,
    )
    if interval is None:
        return None
    low, high, _ = interval
    return [2.0**low, 2.0**high]


def _size_fit(graded: Sequence[TrialOut], seed: int) -> tuple[dict[str, Any] | None, str | None]:
    """Logistic fit of pass on log2(src_lines), with 50% and 80% sizes and task-clustered intervals."""
    rows: list[_FitRow] = []
    for trial in graded:
        replay = _replay_features(trial)
        src = _number(replay.get("src_lines")) if replay is not None else None
        if src is not None and src > 0:
            rows.append(_FitRow(trial.task_id, math.log2(src), 1 if trial.outcome == "pass" else 0))
    if len(rows) < _MIN_FIT_TRIALS:
        return None, f"needs at least {_MIN_FIT_TRIALS} graded trials with src_lines, has {len(rows)}"
    if len({row.y for row in rows}) < 2:
        return None, "needs both pass and fail outcomes"
    fit = fit_logistic([[row.x] for row in rows], [row.y for row in rows])
    x50 = size_at_probability(fit, 0.5)
    x80 = size_at_probability(fit, 0.8)
    stable = fit.converged and fit.coef[1] < 0
    return (
        {
            "intercept": fit.coef[0],
            "slope": fit.coef[1],
            "converged": fit.converged,
            "n": len(rows),
            "x50": x50,
            "x80": x80,
            "lines50": _lines(x50),
            "lines80": _lines(x80),
            "ci50": _lines_interval(rows, 0.5, seed) if stable else None,
            "ci80": _lines_interval(rows, 0.8, seed) if stable else None,
        },
        None,
    )


# ---------------------------------------------------------------------- service


class TrialPlanner(Service):
    # ------------------------------------------------------------------ pool

    def _pool(self, ctx: RequestContext, workspace: str | None, protocol: dict[str, Any]) -> list[_Task]:
        """Replay tasks the staircase may draw from: tagged, validated when configured, with an integer band."""
        spec = _dict(protocol.get("pool"))
        tag = spec.get("tag", POOL_TAG)
        repository = spec.get("repository")
        require_validated = spec.get("require_validated", True)
        found: list[_Task] = []
        offset = 0
        while True:
            page = self.app.tasks.list(
                ctx,
                workspace,
                TaskFilter(tags=[tag], repository=repository, limit=500, offset=offset),
            )
            for task in page.items:
                replay = _dict(task.metadata.get("replay"))
                if not replay or (require_validated and replay.get("validated") is not True):
                    continue
                features = _dict(replay.get("features"))
                band = features.get("band")
                if isinstance(band, int) and not isinstance(band, bool):
                    found.append(_Task(task.key, band, features))
            if page.next_offset is None:
                break
            offset = page.next_offset
        return sorted(found, key=lambda task: task.key)

    def _add(
        self,
        ctx: RequestContext,
        workspace: str | None,
        experiment_ref: str,
        task: _Task,
        arm: str,
        phase: str,
        band: int,
        extra: dict[str, Any] | None = None,
    ) -> TrialOut:
        features: dict[str, Any] = {"replay": task.replay, **(extra or {})}
        return self.app.experiments.add_trial(
            ctx, workspace, experiment_ref, task.key, arm, phase, band=band, features=features
        )

    # ------------------------------------------------------------------ next

    def next(self, ctx: RequestContext, workspace: str | None, experiment_ref: str) -> dict[str, Any]:
        """Return the pending trial, or create the next planned trial, or say why the experiment is done."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        experiment = self.app.experiments.get(ctx, workspace, experiment_ref)
        protocol = experiment.protocol
        primary = _primary(protocol)
        seed = int(protocol.get("seed", 0))
        trials = self.app.experiments.trials(ctx, workspace, experiment_ref)
        step = _step(protocol, trials)

        pending = [t for t in trials if t.status in (TrialStatus.PLANNED, TrialStatus.RUNNING)]
        if pending:
            return _result(False, str(pending[0].phase), pending[0], "pending", step)
        valid = [t for t in trials if t.status != TrialStatus.INVALID]
        if len(valid) >= int(protocol["budget"]["trials"]):
            return _result(True, None, None, "budget", step)
        next_seq = max((t.seq for t in trials), default=0) + 1

        if step.band is not None:
            picked = _nearest_unused(
                self._pool(ctx, workspace, protocol),
                {t.task_key for t in valid if t.arm == primary},
                step.band,
            )
            if picked is None:
                return _result(True, None, None, "pool_exhausted", step)
            band, candidates = picked
            task = _pick(seed, next_seq, candidates)
            extra = {"band_requested": step.band} if band != step.band else None
            trial = self._add(ctx, workspace, experiment_ref, task, primary, step.phase, band, extra)
            return _result(False, step.phase, trial, "planned", step)

        repeats = [t for t in valid if t.phase == "repeat"]
        if len(repeats) < _count(protocol, "repeat"):
            repeated = {t.task_key for t in repeats}
            tasks = [
                task
                for task in _graded_tasks(valid, primary, ("staircase", "refine"))
                if task.key not in repeated
            ]
            if tasks:
                task = _pick(seed, next_seq, tasks)
                trial = self._add(ctx, workspace, experiment_ref, task, primary, "repeat", task.band)
                return _result(False, "repeat", trial, "planned", step)

        control = _dict(protocol.get("control"))
        if isinstance(control.get("arm"), str):
            control_arm = control["arm"]
            ran = [t for t in valid if t.arm == control_arm]
            remaining = _count(protocol, "control") - len(ran)
            if remaining > 0:
                ran_keys = {t.task_key for t in ran}
                available = [task for task in _graded_tasks(valid, primary) if task.key not in ran_keys]
                near: list[_Task] = []
                if step.threshold is not None:
                    centre = round(step.threshold)
                    near = [task for task in available if task.band in (centre, centre + 1)]
                candidates = near if len(near) >= remaining else available
                if candidates:
                    task = _pick(seed, next_seq, candidates)
                    trial = self._add(ctx, workspace, experiment_ref, task, control_arm, "control", task.band)
                    return _result(False, "control", trial, "planned", step)

        return _result(True, None, None, "complete", step)

    # ------------------------------------------------------------------ report

    def report(self, ctx: RequestContext, workspace: str | None, experiment_ref: str) -> dict[str, Any]:
        """Per-arm measurements of an experiment's trials. Every value is JSON-serialisable."""
        require(ctx, Permission.TASK_READ)
        experiment = self.app.experiments.get(ctx, workspace, experiment_ref)
        protocol = experiment.protocol
        trials = self.app.experiments.trials(ctx, workspace, experiment_ref)
        seed = int(protocol.get("seed", 0))
        return {
            "experiment": {"slug": experiment.slug, "status": str(experiment.status), "protocol": protocol},
            "step": _step_dict(_step(protocol, trials)),
            "arms": [self._arm_report(arm["name"], trials, seed) for arm in protocol["arms"]],
        }

    def _arm_report(self, name: str, trials: Sequence[TrialOut], seed: int) -> dict[str, Any]:
        arm_trials = [t for t in trials if t.arm == name]
        valid = [t for t in arm_trials if t.status != TrialStatus.INVALID]
        graded = [t for t in valid if t.status == TrialStatus.GRADED and t.outcome is not None]
        fit, fit_reason = _size_fit(graded, seed)
        causes = Counter(
            (_text(t.failure_cause) or "unclassified")
            for t in arm_trials
            if t.status == TrialStatus.INVALID or (t.status == TrialStatus.GRADED and t.outcome == "fail")
        )
        return {
            "arm": name,
            "trials": {
                "total": len(arm_trials),
                "by_status": _sorted_counts(str(t.status) for t in arm_trials),
                "by_phase": _sorted_counts(str(t.phase) for t in arm_trials),
            },
            "bands": _band_rows(graded),
            "fit": fit,
            "fit_reason": fit_reason,
            "usage": self._usage(graded),
            "failure_causes": dict(sorted(causes.items())),
            "invalid": [
                {
                    "seq": t.seq,
                    "task_key": t.task_key,
                    "failure_cause": _text(t.failure_cause),
                    "notes": t.notes,
                }
                for t in arm_trials
                if t.status == TrialStatus.INVALID
            ],
        }

    def _usage(self, graded: Sequence[TrialOut]) -> dict[str, Any]:
        """Token and turn usage of each outcome's subagent runs; trials without a run are counted apart."""
        usage: dict[str, Any] = {}
        with self.db.session() as session:
            for outcome in ("pass", "fail"):
                runs: list[SubagentRun] = []
                missing = 0
                for trial in graded:
                    if trial.outcome != outcome:
                        continue
                    run = session.get(SubagentRun, trial.subagent_run_id) if trial.subagent_run_id else None
                    if run is None:
                        missing += 1
                    else:
                        runs.append(run)
                entry: dict[str, Any] = {"n": len(runs), "missing_usage": missing}
                for key in _USAGE_KEYS:
                    entry[key] = _summary(
                        v for v in (_number(_dict(r.usage).get(key)) for r in runs) if v is not None
                    )
                for key in _TELEMETRY_KEYS:
                    entry[key] = _summary(
                        v for v in (_number(_dict(r.telemetry).get(key)) for r in runs) if v is not None
                    )
                usage[outcome] = entry
        return usage
