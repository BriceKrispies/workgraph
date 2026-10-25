"""Golden vectors for the pure parts of the experiment statistics (GV-05).

Imports the Python functions of ``workgraph.experiments.stats``, ``staircase``, ``planner`` (pure
helpers) and ``predictor`` (pure helpers), runs them over curated and generated inputs and writes
``golden/expstats/*.json``. Every generated input is stored in the vector, so the Rust side never
needs Python's random generator to reproduce a case. Run with ``uv run python tools/golden/expstats.py``.
"""

from __future__ import annotations

# Seeded random.Random generates the inputs; these are not security-sensitive values.
# ruff: noqa: S311
import json
import math
import random
from collections.abc import Callable
from dataclasses import asdict, is_dataclass
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from workgraph.experiments import planner as pl
from workgraph.experiments import predictor as pr
from workgraph.experiments.staircase import StaircaseConfig, next_step
from workgraph.experiments.stats import (
    bootstrap_interval,
    brier,
    fit_logistic,
    predict,
    reliability,
    size_at_probability,
    wilson,
)

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "expstats"


def encode(value: Any) -> Any:
    """Non-finite floats are not JSON; they are stored as the strings NaN, Infinity and -Infinity."""
    if isinstance(value, float) and not math.isfinite(value):
        return "NaN" if math.isnan(value) else ("Infinity" if value > 0 else "-Infinity")
    if is_dataclass(value) and not isinstance(value, type):
        return encode(asdict(value))
    if isinstance(value, dict):
        return {k: encode(v) for k, v in value.items()}
    if isinstance(value, list | tuple):
        return [encode(v) for v in value]
    return value


def dump(name: str, obj: Any) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text = json.dumps(encode(obj), sort_keys=True, indent=1, ensure_ascii=False, allow_nan=False) + "\n"
    (OUT / name).write_bytes(text.encode("utf-8"))


def jsonable(value: Any) -> Any:
    if is_dataclass(value) and not isinstance(value, type):
        return asdict(value)
    return value


def call(fn: Callable[..., Any], *args: Any, **kwargs: Any) -> dict[str, Any]:
    """Run ``fn`` and record either its result under ``ok`` or the ValueError it raised."""
    try:
        return {"ok": jsonable(fn(*args, **kwargs))}
    except ValueError as exc:
        return {"error": "ValueError", "message": str(exc)}


# --------------------------------------------------------------------------- stats: Wilson


def wilson_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    fixed = [(0, 10), (5, 10), (10, 10), (0, 0), (1, 1), (3, 7), (999, 1000), (1, 3)]
    for successes, n in fixed:
        cases.append(
            {
                "name": f"wilson_{successes}_of_{n}",
                "successes": successes,
                "n": n,
                "z": None,
                "result": call(wilson, successes, n),
            }
        )
    cases.append({"name": "wilson_z_1", "successes": 2, "n": 5, "z": 1.0, "result": call(wilson, 2, 5, 1.0)})
    cases.append(
        {"name": "wilson_z_2_58", "successes": 4, "n": 9, "z": 2.576, "result": call(wilson, 4, 9, 2.576)}
    )
    for successes, n in [(-1, 10), (11, 10), (1, 0), (-1, 0)]:
        cases.append(
            {
                "name": f"wilson_invalid_{successes}_of_{n}",
                "successes": successes,
                "n": n,
                "z": None,
                "result": call(wilson, successes, n),
            }
        )
    rng = random.Random(51)
    for i in range(40):
        n = rng.randint(1, 500)
        successes = rng.randint(0, n)
        cases.append(
            {
                "name": f"wilson_generated_{i:02d}",
                "successes": successes,
                "n": n,
                "z": None,
                "result": call(wilson, successes, n),
            }
        )
    return cases


# --------------------------------------------------------------------------- stats: logistic


def staircase_sample(seed: int, n: int) -> tuple[list[list[float]], list[int]]:
    rng = random.Random(seed)
    x: list[list[float]] = []
    y: list[int] = []
    for _ in range(n):
        value = rng.uniform(0.0, 12.0)
        p = 1.0 / (1.0 + math.exp(-(6.0 - value)))
        x.append([value])
        y.append(1 if rng.random() < p else 0)
    return x, y


def two_feature_sample(seed: int, n: int) -> tuple[list[list[float]], list[int]]:
    rng = random.Random(seed)
    x: list[list[float]] = []
    y: list[int] = []
    for _ in range(n):
        a = rng.uniform(-2.0, 2.0)
        b = rng.uniform(-2.0, 2.0)
        p = 1.0 / (1.0 + math.exp(-(0.5 + 1.5 * a - 2.0 * b)))
        x.append([a, b])
        y.append(1 if rng.random() < p else 0)
    return x, y


def logistic_case(
    name: str,
    covers: list[str],
    x: list[list[float]],
    y: list[int],
    penalty: float | list[float] = 0.0,
    max_iter: int = 100,
    predict_points: list[list[float]] | None = None,
) -> dict[str, Any]:
    result = call(fit_logistic, x, y, penalty=penalty, max_iter=max_iter)
    case: dict[str, Any] = {
        "name": name,
        "covers": covers,
        "x": x,
        "y": y,
        "penalty": penalty,
        "max_iter": max_iter,
        "fit": result,
        "predict": [],
        "size_at_probability": {},
    }
    if "ok" in result:
        fit = fit_logistic(x, y, penalty=penalty, max_iter=max_iter)
        for point in predict_points or []:
            case["predict"].append({"x": point, "p": predict(fit, point)})
        if len(fit.coef) == 2:
            for p in (0.5, 0.8):
                case["size_at_probability"][str(p)] = size_at_probability(fit, p)
    return case


def logistic_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    x, y = staircase_sample(1, 4000)
    cases.append(
        logistic_case(
            "staircase_4000",
            ["test_fit_recovers_known_parameters"],
            x,
            y,
            predict_points=[[0.0], [6.0], [12.0]],
        )
    )
    x, y = staircase_sample(1, 300)
    cases.append(logistic_case("staircase_300_free", ["test_penalty_shrinks_slope_toward_zero"], x, y))
    cases.append(
        logistic_case(
            "staircase_300_penalty_200", ["test_penalty_shrinks_slope_toward_zero"], x, y, penalty=200.0
        )
    )
    cases.append(
        logistic_case(
            "staircase_300_penalty_seq",
            ["test_penalty_sequence_applies_per_coefficient"],
            x,
            y,
            penalty=[0.0, 500.0],
        )
    )
    cases.append(logistic_case("staircase_300_penalty_500", [], x, y, penalty=500.0))
    cases.append(
        logistic_case(
            "separable_no_penalty",
            ["test_separable_data_without_penalty_does_not_converge"],
            [[0.0], [1.0], [2.0], [3.0]],
            [0, 0, 1, 1],
            max_iter=50,
        )
    )
    cases.append(
        logistic_case(
            "separable_penalty_0_1",
            ["test_separable_data_with_penalty_converges"],
            [[0.0], [1.0], [2.0], [3.0]],
            [0, 0, 1, 1],
            penalty=0.1,
        )
    )
    cases.append(
        logistic_case(
            "two_point_penalty_1",
            ["test_predict_is_stable_and_bounded"],
            [[0.0], [1.0]],
            [0, 1],
            penalty=1.0,
            predict_points=[[-1000.0], [1000.0]],
        )
    )
    x, y = two_feature_sample(7, 3000)
    cases.append(
        logistic_case(
            "two_feature_3000",
            ["test_two_feature_fit_and_predict"],
            x,
            y,
            predict_points=[[0.0, 0.0], [1.0, -1.0]],
        )
    )
    cases.append(logistic_case("flat_constant_y", [], [[0.0], [1.0], [2.0]], [1, 1, 1], penalty=0.5))
    for seed in range(1, 9):
        for n in (60, 300):
            x, y = staircase_sample(100 + seed, n)
            pen = 0.0 if seed % 2 else 1.5
            cases.append(
                logistic_case(
                    f"generated_staircase_s{seed}_n{n}", [], x, y, penalty=pen, predict_points=[[3.0], [9.0]]
                )
            )
    for seed in range(1, 5):
        x, y = two_feature_sample(200 + seed, 400)
        cases.append(
            logistic_case(
                f"generated_two_feature_s{seed}",
                [],
                x,
                y,
                penalty=[0.0, 0.2, 0.2],
                predict_points=[[0.0, 0.0], [-1.0, 1.0]],
            )
        )
    return cases


def logistic_error_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    inputs: list[tuple[str, list[list[float]], list[int], Any]] = [
        ("empty", [], [], 0.0),
        ("length_mismatch", [[1.0]], [0, 1], 0.0),
        ("ragged_rows", [[1.0], [1.0, 2.0]], [0, 1], 0.0),
        ("label_two", [[1.0], [2.0]], [0, 2], 0.0),
        ("nan_feature", [[float("nan")], [1.0]], [0, 1], 0.0),
        ("inf_feature", [[float("inf")], [1.0]], [0, 1], 0.0),
        ("penalty_too_short", [[1.0], [2.0]], [0, 1], [1.0]),
        ("penalty_too_long", [[1.0], [2.0]], [0, 1], [0.0, 1.0, 1.0]),
        ("negative_penalty", [[1.0], [2.0]], [0, 1], -1.0),
        ("negative_penalty_seq", [[1.0], [2.0]], [0, 1], [0.0, -0.5]),
    ]
    for name, x, y, pen in inputs:
        cases.append(
            {"name": name, "x": x, "y": y, "penalty": pen, "result": call(fit_logistic, x, y, penalty=pen)}
        )
    cases.append(
        {
            "name": "max_iter_zero",
            "x": [[1.0], [2.0]],
            "y": [0, 1],
            "penalty": 0.0,
            "result": call(fit_logistic, [[1.0], [2.0]], [0, 1], max_iter=0),
        }
    )
    cases.append(
        {
            "name": "size_at_probability_bad_p",
            "p_values": [0.0, 1.0, -0.1, 1.5],
            "results": [
                call(size_at_probability, fit_logistic([[0.0], [1.0]], [0, 1], penalty=1.0), p)
                for p in (0.0, 1.0, -0.1, 1.5)
            ],
        }
    )
    return cases


# --------------------------------------------------------------------------- stats: bootstrap

STATISTICS: dict[str, Callable[[list[Any]], float | None]] = {
    "mean": lambda s: sum(s) / len(s),
    "skip_mod3": lambda s: None if sum(s) % 3 == 0 else float(sum(s)),
    "always_none": lambda s: None,
    "distinct_first_field": lambda s: float(len({row[0] for row in s})),
}


def bootstrap_case(
    name: str,
    covers: list[str],
    rows: list[Any],
    statistic: str,
    cluster_first: bool,
    n: int,
    seed: int,
    level: float = 0.95,
) -> dict[str, Any]:
    cluster = (lambda row: row[0]) if cluster_first else None
    return {
        "name": name,
        "covers": covers,
        "rows": rows,
        "statistic": statistic,
        "cluster_first_field": cluster_first,
        "n": n,
        "seed": seed,
        "level": level,
        "result": call(
            bootstrap_interval, rows, STATISTICS[statistic], cluster=cluster, n=n, seed=seed, level=level
        ),
    }


def bootstrap_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    means = [float(i) for i in range(40)]
    cases.append(
        bootstrap_case(
            "mean_seed_3", ["test_bootstrap_is_deterministic_for_a_seed"], means, "mean", False, 200, 3
        )
    )
    cases.append(
        bootstrap_case(
            "mean_seed_4", ["test_bootstrap_is_deterministic_for_a_seed"], means, "mean", False, 200, 4
        )
    )
    clustered = [[f"task-{i}", j] for i in range(6) for j in range(2)]
    cases.append(
        bootstrap_case(
            "clusters_6x2",
            ["test_bootstrap_resamples_whole_clusters"],
            clustered,
            "distinct_first_field",
            True,
            100,
            0,
        )
    )
    cases.append(
        bootstrap_case(
            "skip_mod3_1_to_10",
            ["test_bootstrap_skips_none_results"],
            [float(i) for i in range(1, 11)],
            "skip_mod3",
            False,
            300,
            0,
        )
    )
    cases.append(
        bootstrap_case(
            "most_none",
            ["test_bootstrap_returns_none_when_most_resamples_fail"],
            [1.0, 2.0],
            "always_none",
            False,
            20,
            0,
        )
    )
    cases.append(
        bootstrap_case("empty_rows", ["test_bootstrap_empty_rows_is_none"], [], "mean", False, 10, 0)
    )
    cases.append(bootstrap_case("invalid_n", [], [1.0], "mean", False, 0, 0))
    cases.append(bootstrap_case("invalid_level", [], [1.0], "mean", False, 10, 0, level=1.0))
    rng = random.Random(77)
    for i in range(20):
        size = rng.randint(1, 60)
        rows = [round(rng.uniform(-5.0, 5.0), 6) for _ in range(size)]
        cases.append(
            bootstrap_case(f"generated_mean_{i:02d}", [], rows, "mean", False, 500, 1000 + i, level=0.9)
        )
    for i in range(6):
        tasks = rng.randint(2, 12)
        rows = [[f"t{rng.randint(0, tasks - 1)}", k] for k in range(rng.randint(3, 40))]
        cases.append(
            bootstrap_case(
                f"generated_clusters_{i:02d}", [], rows, "distinct_first_field", True, 300, 2000 + i
            )
        )
    return cases


# --------------------------------------------------------------------------- stats: calibration


def calibration_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    cases.append(
        {
            "name": "brier_small",
            "covers": ["test_brier_on_small_input"],
            "probs": [0.5, 1.0, 0.0],
            "outcomes": [1, 1, 0],
            "result": call(brier, [0.5, 1.0, 0.0], [1, 1, 0]),
        }
    )
    for name, probs, outcomes in [
        ("empty", [], []),
        ("length_mismatch", [0.5], [1, 0]),
        ("prob_above_one", [1.5], [1]),
        ("outcome_two", [0.5], [2]),
    ]:
        cases.append(
            {
                "name": f"brier_invalid_{name}",
                "covers": [] if name != "empty" else ["test_brier_rejects_bad_input"],
                "probs": probs,
                "outcomes": outcomes,
                "result": call(brier, probs, outcomes),
            }
        )
    cases.append(
        {
            "name": "reliability_edges",
            "covers": ["test_reliability_bins_including_p_equal_one"],
            "probs": [0.05, 0.15, 1.0, 0.95],
            "outcomes": [0, 1, 1, 1],
            "bins": 10,
            "result": call(reliability, [0.05, 0.15, 1.0, 0.95], [0, 1, 1, 1], 10),
        }
    )
    cases.append(
        {
            "name": "reliability_zero_bins",
            "covers": ["test_reliability_rejects_zero_bins"],
            "probs": [0.5],
            "outcomes": [1],
            "bins": 0,
            "result": call(reliability, [0.5], [1], 0),
        }
    )
    rng = random.Random(91)
    for i in range(12):
        n = rng.randint(1, 200)
        probs = [round(rng.random(), 6) for _ in range(n)]
        outcomes = [1 if rng.random() < p else 0 for p in probs]
        bins = (5, 10, 20)[i % 3]
        cases.append(
            {
                "name": f"generated_{i:02d}",
                "covers": [],
                "probs": probs,
                "outcomes": outcomes,
                "bins": bins,
                "brier": call(brier, probs, outcomes),
                "reliability": call(reliability, probs, outcomes, bins),
            }
        )
    return cases


# --------------------------------------------------------------------------- staircase


def protocol_config(overrides: dict[str, Any]) -> dict[str, Any]:
    base: dict[str, Any] = {
        "bands": {"min": 1, "max": 8},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 3, "max_trials": 24},
        "refine": {"trials": 4, "width": 1},
    }
    base.update(overrides)
    return base


def staircase_case(
    name: str,
    covers: list[str],
    config: dict[str, Any],
    staircase: list[list[Any]],
    refine: list[list[Any]] | None = None,
) -> dict[str, Any]:
    cfg = StaircaseConfig(**config)
    hist = [(int(b), bool(p)) for b, p in staircase]
    ref = [(int(b), bool(p)) for b, p in (refine or [])]
    return {
        "name": name,
        "covers": covers,
        "config": config,
        "staircase": staircase,
        "refine": refine or [],
        "result": call(next_step, cfg, hist, ref),
    }


def base_config(**overrides: Any) -> dict[str, Any]:
    values: dict[str, Any] = {
        "min_band": 1,
        "max_band": 8,
        "per_visit": 2,
        "up": 1,
        "down": 2,
        "max_reversals": 3,
        "max_trials": 24,
        "refine_trials": 4,
        "refine_width": 1,
    }
    values.update(overrides)
    return values


def staircase_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    cases.append(staircase_case("starts_at_top", ["test_starts_at_top_band"], base_config(), []))
    cases.append(
        staircase_case(
            "failing_visit",
            ["test_failing_visit_steps_down_by_down"],
            base_config(),
            [[8, False], [8, False]],
        )
    )
    cases.append(
        staircase_case(
            "one_pass_counts",
            ["test_one_pass_in_a_visit_counts_as_passed"],
            base_config(),
            [[4, True], [4, False]],
        )
    )
    cases.append(
        staircase_case(
            "passing_up2", ["test_passing_visit_steps_up_by_up"], base_config(up=2), [[3, True], [3, True]]
        )
    )
    cases.append(
        staircase_case("clamp_top", ["test_clamps_at_the_top"], base_config(), [[8, True], [8, True]])
    )
    cases.append(
        staircase_case(
            "clamp_bottom_2", ["test_clamps_at_the_bottom"], base_config(), [[2, False], [2, False]]
        )
    )
    cases.append(
        staircase_case(
            "clamp_bottom_1", ["test_clamps_at_the_bottom"], base_config(), [[1, False], [1, False]]
        )
    )
    cases.append(
        staircase_case(
            "incomplete_visit", ["test_incomplete_visit_stays_at_its_band"], base_config(), [[5, True]]
        )
    )
    cases.append(
        staircase_case(
            "incomplete_after_complete",
            ["test_incomplete_visit_after_complete_visits_continues_at_its_band"],
            base_config(),
            [[5, True], [5, False], [6, True]],
        )
    )
    cases.append(
        staircase_case(
            "mixed_bands_error",
            ["test_visit_with_mixed_bands_raises"],
            base_config(),
            [[5, True], [6, False]],
        )
    )
    cases.append(
        staircase_case(
            "clamped_reversal",
            ["test_reversal_is_counted_including_clamped_moves"],
            base_config(),
            [[8, True], [8, True], [8, False], [8, False], [6, False], [6, False]],
        )
    )
    cases.append(
        staircase_case(
            "no_reversal",
            ["test_no_reversal_when_direction_is_unchanged"],
            base_config(),
            [[8, False], [8, False], [6, False], [6, False]],
        )
    )
    cases.append(
        staircase_case(
            "threshold_mean_of_reversals",
            ["test_threshold_is_mean_of_reversal_bands"],
            base_config(),
            [[8, False], [8, False], [6, True], [6, True], [7, False], [7, False]],
        )
    )
    cases.append(
        staircase_case(
            "threshold_last_visit",
            ["test_threshold_without_reversal_is_last_visit_band"],
            base_config(),
            [[5, False], [5, False], [3, False], [3, False]],
        )
    )
    cases.append(
        staircase_case(
            "stops_at_max_reversals",
            ["test_stops_at_max_reversals"],
            base_config(max_reversals=2, refine_trials=0),
            [[8, False], [8, False], [6, True], [6, True], [7, False], [7, False]],
        )
    )
    cases.append(
        staircase_case(
            "stops_at_max_trials",
            ["test_stops_at_max_trials"],
            base_config(max_trials=4, refine_trials=0),
            [[5, True], [5, True], [6, False], [6, False]],
        )
    )
    cases.append(
        staircase_case(
            "max_trials_mid_visit",
            ["test_max_trials_ends_the_phase_even_mid_visit"],
            base_config(max_trials=5, refine_trials=0),
            [[5, True], [5, True], [6, False], [6, False], [6, True]],
        )
    )
    cases.append(
        staircase_case(
            "reversal_waits_boundary_done",
            ["test_reversal_stop_waits_for_visit_boundary"],
            base_config(max_reversals=1, refine_trials=0),
            [[8, False], [8, False], [6, True], [6, True]],
        )
    )
    cases.append(
        staircase_case(
            "reversal_waits_boundary_continue",
            ["test_reversal_stop_waits_for_visit_boundary"],
            base_config(max_reversals=1, refine_trials=0),
            [[8, False], [8, False], [6, True], [6, True], [7, False]],
        )
    )
    stair = [[8, False], [8, False], [6, True], [6, True], [7, False], [7, False]]
    cases.append(
        staircase_case(
            "refine_starts", ["test_refine_starts_after_staircase_ends"], base_config(max_reversals=2), stair
        )
    )
    cases.append(
        staircase_case(
            "refine_fewest_trials_a",
            ["test_refine_prefers_band_with_fewest_trials"],
            base_config(max_reversals=2),
            stair,
            [[6, True]],
        )
    )
    cases.append(
        staircase_case(
            "refine_fewest_trials_b",
            ["test_refine_prefers_band_with_fewest_trials"],
            base_config(max_reversals=2),
            stair,
            [[6, True], [7, False], [5, True]],
        )
    )
    cases.append(
        staircase_case(
            "refine_fewest_trials_c",
            ["test_refine_prefers_band_with_fewest_trials"],
            base_config(max_reversals=2),
            stair,
            [[6, True], [7, False]],
        )
    )
    cases.append(
        staircase_case(
            "refine_tie_lower",
            ["test_refine_tie_on_count_and_distance_goes_to_lower_band"],
            base_config(max_reversals=2),
            stair,
            [[5, True]],
        )
    )
    cases.append(
        staircase_case(
            "refine_done",
            ["test_refine_is_done_after_its_trials"],
            base_config(max_reversals=2),
            stair,
            [[6, True], [7, False], [5, True], [6, False]],
        )
    )
    cases.append(
        staircase_case(
            "refine_window_clamped",
            ["test_refine_window_is_clamped_to_band_range"],
            base_config(max_reversals=1, min_band=1, refine_width=2),
            [[2, False], [2, False], [1, True], [1, True]],
        )
    )
    cases.append(
        staircase_case(
            "refine_window_clamped_next",
            ["test_refine_window_is_clamped_to_band_range"],
            base_config(max_reversals=1, min_band=1, refine_width=2),
            [[2, False], [2, False], [1, True], [1, True]],
            [[1, True]],
        )
    )
    cases.append(
        staircase_case(
            "refine_zero_trials",
            ["test_refine_with_zero_trials_is_done"],
            base_config(max_reversals=2, refine_trials=0),
            stair,
        )
    )
    cases.append(
        staircase_case(
            "refine_before_finish_error",
            ["test_refine_before_staircase_finishes_raises"],
            base_config(),
            [[8, False]],
            [[7, True]],
        )
    )
    return cases


def staircase_generated_cases() -> list[dict[str, Any]]:
    """Simulated runs: each run is a seeded config plus a full phase history, with the expected Step
    after every prefix (staircase prefixes first, then refine prefixes)."""
    rng = random.Random(4242)
    runs: list[dict[str, Any]] = []
    for i in range(120):
        min_band = rng.randint(1, 3)
        max_band = rng.randint(min_band + 2, 12)
        per_visit = rng.randint(1, 3)
        config = {
            "min_band": min_band,
            "max_band": max_band,
            "per_visit": per_visit,
            "up": rng.randint(1, 3),
            "down": rng.randint(1, 3),
            "max_reversals": rng.randint(1, 4),
            "max_trials": rng.randint(per_visit, 30),
            "refine_trials": rng.randint(0, 6),
            "refine_width": rng.randint(0, 2),
        }
        cfg = StaircaseConfig(**config)
        slope = rng.uniform(0.5, 1.5)
        midpoint = rng.uniform(min_band, max_band)
        staircase: list[tuple[int, bool]] = []
        refine: list[tuple[int, bool]] = []
        prefixes: list[dict[str, Any]] = []
        for _ in range(200):
            step = next_step(cfg, staircase, refine)
            prefixes.append({"staircase": len(staircase), "refine": len(refine), "step": jsonable(step)})
            if step.phase == "done":
                break
            band = step.band
            if band is None:
                raise RuntimeError("a staircase or refine step always names a band")
            passed = rng.random() < 1.0 / (1.0 + math.exp(-slope * (midpoint - band)))
            if step.phase == "staircase":
                staircase.append((band, passed))
            else:
                refine.append((band, passed))
        runs.append(
            {
                "name": f"simulated_{i:03d}",
                "config": config,
                "staircase": [[b, p] for b, p in staircase],
                "refine": [[b, p] for b, p in refine],
                "prefixes": prefixes,
            }
        )
    return runs


def staircase_config_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    proto = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
        "refine": {"trials": 16, "width": 1},
    }
    cases.append(
        {
            "name": "from_protocol_full",
            "covers": ["test_from_protocol_reads_fields_and_defaults"],
            "protocol": proto,
            "result": call(lambda p: jsonable(StaircaseConfig.from_protocol(p)), proto),
        }
    )
    no_refine = dict(proto)
    del no_refine["refine"]
    cases.append(
        {
            "name": "from_protocol_no_refine",
            "covers": ["test_from_protocol_reads_fields_and_defaults"],
            "protocol": no_refine,
            "result": call(lambda p: jsonable(StaircaseConfig.from_protocol(p)), no_refine),
        }
    )
    missing = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 2, "up": 1, "max_reversals": 6, "max_trials": 24},
    }
    cases.append(
        {
            "name": "from_protocol_missing_down",
            "covers": ["test_from_protocol_names_the_missing_field"],
            "protocol": missing,
            "result": call(lambda p: jsonable(StaircaseConfig.from_protocol(p)), missing),
        }
    )
    invalid = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 0, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
    }
    cases.append(
        {
            "name": "from_protocol_invalid_per_visit",
            "covers": ["test_from_protocol_names_the_invalid_field"],
            "protocol": invalid,
            "result": call(lambda p: jsonable(StaircaseConfig.from_protocol(p)), invalid),
        }
    )
    bool_band = {
        "bands": {"min": True, "max": 9},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
    }
    cases.append(
        {
            "name": "from_protocol_bool_band",
            "covers": [],
            "protocol": bool_band,
            "result": call(lambda p: jsonable(StaircaseConfig.from_protocol(p)), bool_band),
        }
    )
    for name, kwargs in [
        ("min_above_max", {"min_band": 9, "max_band": 3}),
        ("max_trials_below_visit", {"max_trials": 1, "per_visit": 2}),
        ("zero_per_visit", {"per_visit": 0}),
        ("negative_refine_width", {"refine_width": -1}),
        ("zero_refine_trials_ok", {"refine_trials": 0}),
    ]:
        cfg = base_config(**kwargs)
        covers = (
            ["test_config_rejects_min_above_max"]
            if name == "min_above_max"
            else (
                ["test_config_rejects_max_trials_below_one_visit"] if name == "max_trials_below_visit" else []
            )
        )
        cases.append(
            {
                "name": f"config_{name}",
                "covers": covers,
                "config": cfg,
                "result": call(lambda c: jsonable(StaircaseConfig(**c)), cfg),
            }
        )
    return cases


# --------------------------------------------------------------------------- planner (pure helpers)


def planner_cases() -> dict[str, Any]:
    out: dict[str, Any] = {}
    summary: list[dict[str, Any]] = []
    rng = random.Random(606)
    for values in [[], [1.0], [1.0, 2.0], [3.0, 1.0, 2.0, 10.0], [0.1, 0.2, 0.3, 0.4, 0.5, 0.6]]:
        summary.append({"values": values, "result": call(pl._summary, values)})
    for _ in range(20):
        values = [round(rng.uniform(-10, 10), 6) for _ in range(rng.randint(1, 15))]
        summary.append({"values": values, "result": call(pl._summary, values)})
    out["summary"] = summary

    counts: list[dict[str, Any]] = []
    for values in [
        [],
        ["pass"],
        ["fail", "pass", "fail"],
        ["b", "a", "c", "a"],
        ["tool_error", "timeout", "tool_error", "", "pass"],
    ]:
        counts.append({"values": values, "result": call(pl._sorted_counts, values)})
    out["sorted_counts"] = counts

    band_rows: list[dict[str, Any]] = []
    scenarios = [
        [],
        [(3, "pass"), (3, "fail"), (1, "pass"), (None, "pass")],
        [(8, "fail")] * 4,
        [(2, "pass")] * 7 + [(2, "fail")] * 3,
    ]
    for _ in range(10):
        rows = []
        for _ in range(rng.randint(1, 30)):
            band = rng.choice([None, 1, 2, 3, 5, 8, 9])
            rows.append((band, rng.choice(["pass", "fail"])))
        scenarios.append(rows)
    for rows in scenarios:
        graded = [SimpleNamespace(band=b, outcome=o) for b, o in rows]
        band_rows.append(
            {
                "trials": [{"band": b, "outcome": o} for b, o in rows],
                "result": call(pl._band_rows, graded),
            }
        )
    out["band_rows"] = band_rows

    lines: list[dict[str, Any]] = []
    for x in [None, 0.0, 1.0, 2.5, -3.0, 10.25, 1e-9]:
        lines.append({"x": x, "result": call(pl._lines, x)})
    out["lines"] = lines

    intervals: list[dict[str, Any]] = []
    for seed in (0, 1, 7):
        for p in (0.5, 0.8):
            fit_rows = []
            rng2 = random.Random(seed * 10 + int(p * 10))
            for t in range(12):
                x = rng2.uniform(0.0, 8.0)
                for _ in range(3):
                    y = 1 if rng2.random() < 1.0 / (1.0 + math.exp(-(4.0 - x))) else 0
                    fit_rows.append({"task_id": f"task-{t}", "x": x, "y": y})
            rows_obj = [pl._FitRow(r["task_id"], r["x"], r["y"]) for r in fit_rows]
            intervals.append(
                {
                    "rows": fit_rows,
                    "p": p,
                    "seed": seed,
                    "result": call(pl._lines_interval, rows_obj, p, seed),
                }
            )
    out["lines_interval"] = intervals

    size_fits: list[dict[str, Any]] = []
    fixtures: list[list[dict[str, Any]]] = [
        [],
        [{"task_id": "t1", "outcome": "pass", "src": 10.0}] * 3,
        [
            {"task_id": "t1", "outcome": "pass", "src": 10.0},
            {"task_id": "t2", "outcome": "fail", "src": 200.0},
            {"task_id": "t3", "outcome": "pass", "src": 0.0},
        ],
        [
            {"task_id": f"t{i}", "outcome": "pass" if i < 5 else "fail", "src": float(10 + 3 * i)}
            for i in range(10)
        ],
    ]
    for _ in range(10):
        rows = []
        for t in range(rng.randint(4, 20)):
            src = float(rng.randint(5, 900))
            p_pass = 1.0 / (1.0 + math.exp((math.log2(src) - 7.0) * 1.5))
            rows.append(
                {"task_id": f"g{t % 9}", "outcome": "pass" if rng.random() < p_pass else "fail", "src": src}
            )
        fixtures.append(rows)
    for rows in fixtures:
        graded = [
            SimpleNamespace(
                task_id=r["task_id"], outcome=r["outcome"], features={"replay": {"src_lines": r["src"]}}
            )
            for r in rows
        ]
        size_fits.append(
            {"trials": rows, "seed": 5, "result": call(lambda g: list(pl._size_fit(g, 5)), graded)}
        )
    out["size_fit"] = size_fits

    picks: list[dict[str, Any]] = []
    for seed in (0, 1, 42):
        for next_seq in (0, 1, 7, 100):
            tasks = [{"key": f"task-{k:02d}", "band": k % 4} for k in (3, 1, 9, 0, 5, 12)]
            picks.append(
                {
                    "seed": seed,
                    "next_seq": next_seq,
                    "tasks": tasks,
                    "result": call(
                        lambda s, n, ts: pl._pick(s, n, [pl._Task(t["key"], t["band"], {}) for t in ts]).key,
                        seed,
                        next_seq,
                        tasks,
                    ),
                }
            )
    out["pick"] = picks

    nearest: list[dict[str, Any]] = []
    pool = [{"key": f"k{i}", "band": b} for i, b in enumerate([1, 3, 3, 5, 8, 8, 8, 10])]
    for used in (set(), {"k1"}, {"k1", "k2", "k3"}, {f"k{i}" for i in range(8)}):
        for band in (0, 3, 4, 7, 9, 20):
            nearest.append(
                {
                    "used": sorted(used),
                    "band": band,
                    "pool": pool,
                    "result": call(_nearest, used, band, pool),
                }
            )
    out["nearest_unused"] = nearest
    return out


def _nearest(used: set[str], band: int, pool: list[dict[str, Any]]) -> Any:
    found = pl._nearest_unused([pl._Task(t["key"], t["band"], {}) for t in pool], used, band)
    if found is None:
        return None
    chosen, tasks = found
    return [chosen, [t.key for t in tasks]]


# --------------------------------------------------------------------------- predictor (pure helpers)


def predictor_cases() -> dict[str, Any]:
    out: dict[str, Any] = {}
    feats: list[dict[str, Any]] = []
    for lines, files, pkgs in [
        (0.0, 0.0, 0),
        (0.5, 1.0, 1),
        (1.0, 2.0, 3),
        (1000.0, 40.0, 7),
        (1e6, 3.5, 0),
        (3.5, 0.25, 12),
        (-4.0, 9.0, 2),
    ]:
        feats.append(
            {
                "src_lines": lines,
                "src_files": files,
                "packages": pkgs,
                "result": call(pr.features_of, lines, files, pkgs),
            }
        )
    out["features_of"] = feats
    prob: list[dict[str, Any]] = []
    for coef, x in [
        ([0.0], []),
        ([0.5, 1.5, -2.0], [0.0, 0.0]),
        ([-800.0], []),
        ([800.0], []),
        ([-1.0, 2.0], [-3.0]),
        ([6.0, -1.0], [12.0]),
        ([0.0, 0.25], [1e6]),
    ]:
        prob.append({"coef": coef, "x": x, "result": call(pr._probability, coef, x)})
    out["probability"] = prob
    pct: list[dict[str, Any]] = []
    rng = random.Random(808)
    for values in [[1.0], [1.0, 2.0], [0.0, 10.0, 20.0, 30.0]]:
        for q in (0.0, 0.1, 0.25, 0.5, 0.9, 1.0):
            pct.append({"values": values, "q": q, "result": call(pr._percentile, values, q)})
    for _ in range(10):
        values = sorted(round(rng.uniform(-50, 50), 6) for _ in range(rng.randint(1, 40)))
        q = round(rng.random(), 6)
        pct.append({"values": values, "q": q, "result": call(pr._percentile, values, q)})
    out["percentile"] = pct
    return out


def main() -> None:
    dump("stats_wilson.json", {"cases": wilson_cases()})
    dump("stats_logistic.json", {"cases": logistic_cases(), "errors": logistic_error_cases()})
    dump("stats_bootstrap.json", {"cases": bootstrap_cases(), "statistics": sorted(STATISTICS)})
    dump("stats_calibration.json", {"cases": calibration_cases()})
    dump(
        "staircase.json",
        {
            "config_cases": staircase_config_cases(),
            "cases": staircase_cases(),
            "simulated": staircase_generated_cases(),
        },
    )
    dump("planner.json", planner_cases())
    dump("predictor.json", predictor_cases())


if __name__ == "__main__":
    main()
