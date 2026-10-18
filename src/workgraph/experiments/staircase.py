"""Band staircase: start at the top band, step by visits, then refine around the threshold.

Trials are grouped into consecutive visits of ``per_visit`` trials at one band. A visit passes
when at least half of its trials pass; a pass moves the band up, a failure moves it down, both
clamped to the band range. The staircase ends after enough direction reversals or trials. The
threshold is the mean band of the reversal visits, and the refine phase spends its trials on the
bands around that threshold. ``next_step`` is pure: it recomputes everything from the history.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Literal


def _int_field(value: Any, name: str, minimum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{name} must be an integer, got {value!r}")
    if value < minimum:
        raise ValueError(f"{name} must be >= {minimum}, got {value}")
    return value


@dataclass(frozen=True)
class StaircaseConfig:
    """Band range and stopping rules for one staircase."""

    min_band: int
    max_band: int
    per_visit: int
    up: int
    down: int
    max_reversals: int
    max_trials: int
    refine_trials: int = 0
    refine_width: int = 1

    def __post_init__(self) -> None:
        _int_field(self.min_band, "min_band", 1)
        _int_field(self.max_band, "max_band", 1)
        if self.min_band > self.max_band:
            raise ValueError(f"min_band ({self.min_band}) must be <= max_band ({self.max_band})")
        _int_field(self.per_visit, "per_visit", 1)
        _int_field(self.up, "up", 1)
        _int_field(self.down, "down", 1)
        _int_field(self.max_reversals, "max_reversals", 1)
        _int_field(self.max_trials, "max_trials", 1)
        if self.max_trials < self.per_visit:
            raise ValueError(f"max_trials ({self.max_trials}) must be >= per_visit ({self.per_visit})")
        _int_field(self.refine_trials, "refine_trials", 0)
        _int_field(self.refine_width, "refine_width", 0)

    @classmethod
    def from_protocol(cls, protocol: Mapping[str, Any]) -> StaircaseConfig:
        """Build the config from an experiment protocol mapping."""
        try:
            bands = protocol["bands"]
            staircase = protocol["staircase"]
            min_band = bands["min"]
            max_band = bands["max"]
            per_visit = staircase["per_visit"]
            up = staircase["up"]
            down = staircase["down"]
            max_reversals = staircase["max_reversals"]
            max_trials = staircase["max_trials"]
        except (KeyError, TypeError) as exc:
            raise ValueError(f"protocol is missing a required field: {exc}") from exc
        refine = protocol.get("refine", {})
        return cls(
            min_band=_int_field(min_band, "bands.min", 1),
            max_band=_int_field(max_band, "bands.max", 1),
            per_visit=_int_field(per_visit, "staircase.per_visit", 1),
            up=_int_field(up, "staircase.up", 1),
            down=_int_field(down, "staircase.down", 1),
            max_reversals=_int_field(max_reversals, "staircase.max_reversals", 1),
            max_trials=_int_field(max_trials, "staircase.max_trials", 1),
            refine_trials=_int_field(refine.get("trials", 0), "refine.trials", 0),
            refine_width=_int_field(refine.get("width", 1), "refine.width", 0),
        )


@dataclass(frozen=True)
class Step:
    """What to do next: the phase, the band for the next trial, and the state behind it."""

    phase: Literal["staircase", "refine", "done"]
    band: int | None
    reversals: int
    threshold: float | None
    visits: tuple[tuple[int, int, int], ...]


@dataclass(frozen=True)
class _Visit:
    band: int
    passes: int
    trials: int
    complete: bool

    @property
    def passed(self) -> bool:
        return self.passes * 2 >= self.trials


def _group_visits(config: StaircaseConfig, history: Sequence[tuple[int, bool]]) -> list[_Visit]:
    visits: list[_Visit] = []
    for start in range(0, len(history), config.per_visit):
        chunk = history[start : start + config.per_visit]
        bands = {band for band, _ in chunk}
        if len(bands) > 1:
            raise ValueError(f"visit starting at trial {start} mixes bands {sorted(bands)}")
        visits.append(
            _Visit(
                band=chunk[0][0],
                passes=sum(1 for _, passed in chunk if passed),
                trials=len(chunk),
                complete=len(chunk) == config.per_visit,
            )
        )
    return visits


def _reversal_visits(completed: Sequence[_Visit]) -> list[_Visit]:
    """Completed visits whose direction differs from the previous completed visit's direction."""
    reversal_visits: list[_Visit] = []
    previous: bool | None = None
    for visit in completed:
        if previous is not None and visit.passed != previous:
            reversal_visits.append(visit)
        previous = visit.passed
    return reversal_visits


def _staircase_ended(config: StaircaseConfig, staircase: Sequence[tuple[int, bool]], reversals: int) -> bool:
    if len(staircase) >= config.max_trials:
        return True
    at_boundary = len(staircase) % config.per_visit == 0
    return at_boundary and reversals >= config.max_reversals


def _current_band(config: StaircaseConfig, visits: Sequence[_Visit]) -> int:
    if not visits:
        return config.max_band
    last = visits[-1]
    if not last.complete:
        return last.band
    delta = config.up if last.passed else -config.down
    return min(config.max_band, max(config.min_band, last.band + delta))


def next_step(
    config: StaircaseConfig,
    staircase: Sequence[tuple[int, bool]],
    refine: Sequence[tuple[int, bool]] = (),
) -> Step:
    """Return the next phase and band given the ordered trial history of each phase."""
    visits = _group_visits(config, staircase)
    completed = [v for v in visits if v.complete]
    reversal_visits = _reversal_visits(completed)
    reversals = len(reversal_visits)
    ended = _staircase_ended(config, staircase, reversals)
    if refine and not ended:
        raise ValueError("refine trials were given before the staircase phase finished")

    threshold: float | None
    if reversal_visits:
        threshold = sum(v.band for v in reversal_visits) / reversals
    elif completed:
        threshold = float(completed[-1].band)
    else:
        threshold = None
    visit_summary = tuple((v.band, v.passes, v.trials) for v in completed)

    if not ended:
        return Step(
            phase="staircase",
            band=_current_band(config, visits),
            reversals=reversals,
            threshold=threshold,
            visits=visit_summary,
        )

    if len(refine) < config.refine_trials and threshold is not None:
        center = round(threshold)
        low = max(config.min_band, center - config.refine_width)
        high = min(config.max_band, center + config.refine_width)
        counts = dict.fromkeys(range(low, high + 1), 0)
        for band, _ in refine:
            if band in counts:
                counts[band] += 1
        band = min(counts, key=lambda b: (counts[b], abs(b - threshold), b))
        return Step(
            phase="refine",
            band=band,
            reversals=reversals,
            threshold=threshold,
            visits=visit_summary,
        )

    return Step(phase="done", band=None, reversals=reversals, threshold=threshold, visits=visit_summary)
