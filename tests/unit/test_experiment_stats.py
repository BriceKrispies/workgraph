"""Tests for the standard-library experiment statistics."""

from __future__ import annotations

import math
import random
from collections import Counter
from collections.abc import Sequence

import pytest

from workgraph.experiments.stats import (
    LogisticFit,
    ReliabilityBin,
    bootstrap_interval,
    brier,
    fit_logistic,
    predict,
    reliability,
    size_at_probability,
    wilson,
)


def _staircase_sample(seed: int = 1, n: int = 4000) -> tuple[list[list[float]], list[int]]:
    rng = random.Random(seed)
    x: list[list[float]] = []
    y: list[int] = []
    for _ in range(n):
        value = rng.uniform(0.0, 12.0)
        p = 1.0 / (1.0 + math.exp(-(6.0 - value)))
        x.append([value])
        y.append(1 if rng.random() < p else 0)
    return x, y


# Wilson ---------------------------------------------------------------------------------------


def test_wilson_zero_successes() -> None:
    # With no successes the upper bound has the closed form z^2 / (n + z^2).
    z2 = 1.959963984540054**2
    lo, hi = wilson(0, 10)
    assert lo == 0.0
    assert hi == pytest.approx(z2 / (10 + z2), abs=1e-6)
    assert hi == pytest.approx(0.2775328, abs=1e-6)


def test_wilson_half_successes() -> None:
    lo, hi = wilson(5, 10)
    assert lo == pytest.approx(0.236593, abs=1e-6)
    assert hi == pytest.approx(0.763407, abs=1e-6)


def test_wilson_all_successes() -> None:
    lo, hi = wilson(10, 10)
    # With all successes the lower bound has the closed form n / (n + z^2).
    z2 = 1.959963984540054**2
    assert lo == pytest.approx(10 / (10 + z2), abs=1e-6)
    assert lo == pytest.approx(0.7224672, abs=1e-6)
    assert hi == pytest.approx(1.0, abs=1e-12)


def test_wilson_empty_sample_is_uninformative() -> None:
    assert wilson(0, 0) == (0.0, 1.0)


@pytest.mark.parametrize(("successes", "n"), [(-1, 10), (11, 10), (1, 0), (-1, 0)])
def test_wilson_rejects_invalid_counts(successes: int, n: int) -> None:
    with pytest.raises(ValueError):
        wilson(successes, n)


# Logistic regression --------------------------------------------------------------------------


def test_fit_recovers_known_parameters() -> None:
    x, y = _staircase_sample()
    fit = fit_logistic(x, y)
    assert fit.converged
    assert fit.n == 4000
    # Tolerances are about 3 theoretical standard errors (0.20 intercept, 0.03 slope); 0.15 is below one SE.
    assert fit.coef[0] == pytest.approx(6.0, abs=0.6)
    assert fit.coef[1] == pytest.approx(-1.0, abs=0.1)
    assert size_at_probability(fit, 0.5) == pytest.approx(6.0, abs=0.3)


def test_penalty_shrinks_slope_toward_zero() -> None:
    x, y = _staircase_sample(n=300)
    free = fit_logistic(x, y)
    shrunk = fit_logistic(x, y, penalty=200.0)
    assert free.converged and shrunk.converged
    assert abs(shrunk.coef[1]) < abs(free.coef[1])
    assert shrunk.penalty == (0.0, 200.0)
    assert free.penalty == (0.0, 0.0)


def test_penalty_sequence_applies_per_coefficient() -> None:
    x, y = _staircase_sample(n=300)
    fit = fit_logistic(x, y, penalty=[0.0, 500.0])
    assert fit.penalty == (0.0, 500.0)
    assert abs(fit.coef[1]) < abs(fit_logistic(x, y).coef[1])


def test_penalty_sequence_must_cover_intercept() -> None:
    x, y = _staircase_sample(n=50)
    with pytest.raises(ValueError, match="penalty"):
        fit_logistic(x, y, penalty=[1.0])
    with pytest.raises(ValueError, match="penalty"):
        fit_logistic(x, y, penalty=[0.0, 1.0, 1.0])


def test_negative_penalty_rejected() -> None:
    x, y = _staircase_sample(n=50)
    with pytest.raises(ValueError):
        fit_logistic(x, y, penalty=-1.0)


def test_separable_data_without_penalty_does_not_converge() -> None:
    x = [[0.0], [1.0], [2.0], [3.0]]
    y = [0, 0, 1, 1]
    fit = fit_logistic(x, y, max_iter=50)
    assert fit.converged is False
    assert fit.iterations <= 50
    assert fit.coef[1] > 10  # the slope keeps growing: no finite maximum exists


def test_separable_data_with_penalty_converges() -> None:
    x = [[0.0], [1.0], [2.0], [3.0]]
    y = [0, 0, 1, 1]
    fit = fit_logistic(x, y, penalty=0.1)
    assert fit.converged
    midpoint = size_at_probability(fit, 0.5)
    assert midpoint is not None
    assert 1.0 < midpoint < 2.0


def test_two_feature_fit_and_predict() -> None:
    rng = random.Random(7)
    x: list[list[float]] = []
    y: list[int] = []
    for _ in range(3000):
        a = rng.uniform(-2.0, 2.0)
        b = rng.uniform(-2.0, 2.0)
        p = 1.0 / (1.0 + math.exp(-(0.5 + 1.5 * a - 2.0 * b)))
        x.append([a, b])
        y.append(1 if rng.random() < p else 0)
    fit = fit_logistic(x, y)
    assert fit.converged
    assert len(fit.coef) == 3
    assert fit.coef[0] == pytest.approx(0.5, abs=0.15)
    assert fit.coef[1] == pytest.approx(1.5, abs=0.15)
    assert fit.coef[2] == pytest.approx(-2.0, abs=0.15)
    assert size_at_probability(fit, 0.5) is None
    assert predict(fit, [0.0, 0.0]) == pytest.approx(1.0 / (1.0 + math.exp(-0.5)), abs=0.05)


def test_predict_is_stable_and_bounded() -> None:
    fit = fit_logistic([[0.0], [1.0]], [0, 1], penalty=1.0)
    assert 0.0 < predict(fit, [-1000.0]) < 1e-12
    assert 1.0 - 1e-12 < predict(fit, [1000.0]) <= 1.0


def test_predict_checks_feature_count() -> None:
    fit = fit_logistic([[0.0], [1.0]], [0, 1], penalty=1.0)
    with pytest.raises(ValueError):
        predict(fit, [1.0, 2.0])


def test_size_at_probability_is_none_when_slope_is_zero() -> None:
    flat = LogisticFit(
        coef=(0.3, 0.0), n=4, converged=True, iterations=1, log_likelihood=0.0, penalty=(0.0, 0.0)
    )
    assert size_at_probability(flat, 0.5) is None


def test_size_at_probability_rejects_bad_p() -> None:
    fit = fit_logistic([[0.0], [1.0]], [0, 1], penalty=1.0)
    for bad in (0.0, 1.0, -0.1, 1.5):
        with pytest.raises(ValueError):
            size_at_probability(fit, bad)


@pytest.mark.parametrize(
    ("x", "y"),
    [
        ([], []),
        ([[1.0]], [0, 1]),
        ([[1.0], [1.0, 2.0]], [0, 1]),
        ([[1.0], [2.0]], [0, 2]),
    ],
)
def test_fit_rejects_invalid_input(x: list[list[float]], y: list[int]) -> None:
    with pytest.raises(ValueError):
        fit_logistic(x, y)


# Bootstrap ------------------------------------------------------------------------------------


def _mean_statistic(sample: Sequence[float]) -> float:
    return sum(sample) / len(sample)


def test_bootstrap_is_deterministic_for_a_seed() -> None:
    rows = [float(i) for i in range(40)]
    first = bootstrap_interval(rows, _mean_statistic, n=200, seed=3)
    second = bootstrap_interval(rows, _mean_statistic, n=200, seed=3)
    other = bootstrap_interval(rows, _mean_statistic, n=200, seed=4)
    assert first == second
    assert first is not None and other is not None
    assert first[2] == 200
    assert first[0] < 19.5 < first[1]
    assert other != first


def test_bootstrap_resamples_whole_clusters() -> None:
    rows = [(f"task-{i}", j) for i in range(6) for j in range(2)]
    seen: list[tuple[int, bool]] = []

    def statistic(sample: Sequence[tuple[str, int]]) -> float:
        counts = Counter(task for task, _ in sample)
        seen.append((len(sample), all(c % 2 == 0 for c in counts.values())))
        return float(len(counts))

    result = bootstrap_interval(rows, statistic, cluster=lambda r: r[0], n=100, seed=0)
    assert result is not None
    # Six clusters of two rows are drawn per resample, so every sample has 12 rows, and each task
    # appears an even number of times because its two rows are always drawn together.
    assert len(seen) == 100
    assert all(length == 12 and all_whole for length, all_whole in seen)


def test_bootstrap_skips_none_results() -> None:
    rows = [float(i) for i in range(1, 11)]

    def statistic(sample: Sequence[float]) -> float | None:
        total = sum(sample)
        return None if total % 3 == 0 else float(total)

    result = bootstrap_interval(rows, statistic, n=300, seed=0)
    assert result is not None
    lo, hi, produced = result
    assert 150 < produced < 300
    assert lo <= hi


def test_bootstrap_returns_none_when_most_resamples_fail() -> None:
    assert bootstrap_interval([1.0, 2.0], lambda _s: None, n=20, seed=0) is None


def test_bootstrap_empty_rows_is_none() -> None:
    assert bootstrap_interval([], _mean_statistic, n=10) is None


# Calibration ----------------------------------------------------------------------------------


def test_brier_on_small_input() -> None:
    assert brier([0.5, 1.0, 0.0], [1, 1, 0]) == pytest.approx(0.25 / 3)


def test_brier_rejects_bad_input() -> None:
    with pytest.raises(ValueError):
        brier([], [])
    with pytest.raises(ValueError):
        brier([0.5], [1, 0])
    with pytest.raises(ValueError):
        brier([1.5], [1])


def test_reliability_bins_including_p_equal_one() -> None:
    bins = reliability([0.05, 0.15, 1.0, 0.95], [0, 1, 1, 1], bins=10)
    assert len(bins) == 10
    assert bins[0] == ReliabilityBin(lo=0.0, hi=0.1, n=1, mean_predicted=0.05, observed=0.0)
    assert bins[1].n == 1 and bins[1].observed == 1.0
    assert bins[9].n == 2
    assert bins[9].mean_predicted == pytest.approx(0.975)
    assert bins[9].observed == 1.0
    assert bins[5] == ReliabilityBin(lo=0.5, hi=0.6, n=0, mean_predicted=None, observed=None)
    assert sum(b.n for b in bins) == 4


def test_reliability_rejects_zero_bins() -> None:
    with pytest.raises(ValueError):
        reliability([0.5], [1], bins=0)
