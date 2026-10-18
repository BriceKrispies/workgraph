"""Standard-library statistics for capability experiments.

Wilson intervals for pass rates, penalised logistic regression fitted by Newton-Raphson,
percentile bootstrap intervals (optionally resampling whole clusters), and calibration
measures. Everything is deterministic for a given input and seed.
"""

from __future__ import annotations

import math
from collections.abc import Callable, Hashable, Sequence
from dataclasses import dataclass
from random import Random

_Z_95 = 1.959963984540054
_STEP_HALVINGS = 30
_SLACK = 1e-10


def wilson(successes: int, n: int, z: float = _Z_95) -> tuple[float, float]:
    """Wilson score interval for a binomial proportion. ``(0.0, 1.0)`` when ``n`` is 0."""
    if n < 0:
        raise ValueError(f"n must be >= 0, got {n}")
    if successes < 0 or successes > n:
        raise ValueError(f"successes must be in [0, {n}], got {successes}")
    if n == 0:
        return (0.0, 1.0)
    p_hat = successes / n
    z2 = z * z
    denom = 1.0 + z2 / n
    center = (p_hat + z2 / (2.0 * n)) / denom
    half = z * math.sqrt(p_hat * (1.0 - p_hat) / n + z2 / (4.0 * n * n)) / denom
    return (max(0.0, center - half), min(1.0, center + half))


@dataclass(frozen=True)
class LogisticFit:
    """A fitted logistic model. ``coef[0]`` is the intercept; ``log_likelihood`` is unpenalised."""

    coef: tuple[float, ...]
    n: int
    converged: bool
    iterations: int
    log_likelihood: float
    penalty: tuple[float, ...]


def _dot(a: Sequence[float], b: Sequence[float]) -> float:
    return sum(ai * bi for ai, bi in zip(a, b, strict=True))


def _sigmoid(eta: float) -> float:
    if eta >= 0:
        return 1.0 / (1.0 + math.exp(-eta))
    e = math.exp(eta)
    return e / (1.0 + e)


def _log1pexp(eta: float) -> float:
    """``log(1 + exp(eta))`` without overflow."""
    return max(eta, 0.0) + math.log1p(math.exp(-abs(eta)))


def _log_likelihood(
    rows: Sequence[Sequence[float]], targets: Sequence[float], beta: Sequence[float]
) -> float:
    total = 0.0
    for row, yi in zip(rows, targets, strict=True):
        eta = _dot(row, beta)
        total += yi * eta - _log1pexp(eta)
    return total


def _solve(matrix: list[list[float]], rhs: list[float]) -> list[float] | None:
    """Solve ``matrix @ x = rhs`` by Gaussian elimination with partial pivoting; None if singular."""
    size = len(rhs)
    a = [[*row, rhs[i]] for i, row in enumerate(matrix)]
    for col in range(size):
        pivot = max(range(col, size), key=lambda r: abs(a[r][col]))
        if abs(a[pivot][col]) <= 1e-300:
            return None
        a[col], a[pivot] = a[pivot], a[col]
        for r in range(col + 1, size):
            factor = a[r][col] / a[col][col]
            if factor != 0.0:
                for c in range(col, size + 1):
                    a[r][c] -= factor * a[col][c]
    x = [0.0] * size
    for r in range(size - 1, -1, -1):
        total = a[r][size] - sum(a[r][c] * x[c] for c in range(r + 1, size))
        x[r] = total / a[r][r]
    return x


def fit_logistic(
    x: Sequence[Sequence[float]],
    y: Sequence[int],
    *,
    penalty: float | Sequence[float] = 0.0,
    max_iter: int = 100,
    tol: float = 1e-9,
) -> LogisticFit:
    """Fit ``P(y=1) = sigmoid(b0 + b . x)`` by penalised maximum likelihood.

    The objective is ``sum(log lik) - 0.5 * sum(penalty_j * coef_j**2)``. A float penalty applies
    to every feature coefficient (the intercept is unpenalised); a sequence gives one value per
    coefficient, intercept first. Separable data with no penalty does not converge: the fit stops
    at ``max_iter`` with ``converged=False``.
    """
    if len(x) == 0:
        raise ValueError("fit_logistic needs at least one row")
    if len(x) != len(y):
        raise ValueError(f"x has {len(x)} rows but y has {len(y)} values")
    if max_iter < 1:
        raise ValueError(f"max_iter must be >= 1, got {max_iter}")
    width = len(x[0])
    if any(len(row) != width for row in x):
        raise ValueError("all rows of x must have the same number of features")
    if any(not math.isfinite(v) for row in x for v in row):
        raise ValueError("feature values must be finite")
    if any(yi not in (0, 1) for yi in y):
        raise ValueError("y values must be 0 or 1")

    size = width + 1
    if isinstance(penalty, int | float):
        pen = [0.0] + [float(penalty)] * width
    else:
        pen = [float(v) for v in penalty]
        if len(pen) != size:
            raise ValueError(f"penalty needs {size} values (intercept first), got {len(pen)}")
    if any(v < 0 for v in pen):
        raise ValueError("penalty values must be >= 0")

    rows = [[1.0, *(float(v) for v in row)] for row in x]
    targets = [float(yi) for yi in y]

    def objective(beta: list[float]) -> float:
        penalty_term = 0.5 * sum(p * b * b for p, b in zip(pen, beta, strict=True))
        return _log_likelihood(rows, targets, beta) - penalty_term

    beta = [0.0] * size
    current = objective(beta)
    iterations = 0
    converged = False
    for _ in range(max_iter):
        grad = [-pen[j] * beta[j] for j in range(size)]
        hess = [[0.0] * size for _ in range(size)]
        for row, yi in zip(rows, targets, strict=True):
            mu = _sigmoid(_dot(row, beta))
            weight = mu * (1.0 - mu)
            residual = yi - mu
            for j in range(size):
                grad[j] += residual * row[j]
                for k in range(size):
                    hess[j][k] += weight * row[j] * row[k]
        for j in range(size):
            hess[j][j] += pen[j]
        step = _solve(hess, grad)
        if step is None:
            break
        iterations += 1
        if max(abs(s) for s in step) <= tol:
            beta = [b + s for b, s in zip(beta, step, strict=True)]
            converged = True
            break
        # Step halving: accept the first step that does not lower the objective (up to rounding).
        scale = 1.0
        accepted = False
        for _ in range(_STEP_HALVINGS):
            candidate = [b + scale * s for b, s in zip(beta, step, strict=True)]
            value = objective(candidate)
            if value >= current - _SLACK * (1.0 + abs(current)):
                beta, current = candidate, value
                accepted = True
                break
            scale /= 2.0
        if not accepted:
            break

    return LogisticFit(
        coef=tuple(beta),
        n=len(y),
        converged=converged,
        iterations=iterations,
        log_likelihood=_log_likelihood(rows, targets, beta),
        penalty=tuple(pen),
    )


def predict(fit: LogisticFit, x: Sequence[float]) -> float:
    """Predicted probability of success for one feature vector."""
    if len(x) != len(fit.coef) - 1:
        raise ValueError(f"expected {len(fit.coef) - 1} features, got {len(x)}")
    return _sigmoid(fit.coef[0] + _dot(fit.coef[1:], [float(v) for v in x]))


def size_at_probability(fit: LogisticFit, p: float) -> float | None:
    """The feature value where the predicted probability equals ``p`` (single-feature fits only)."""
    if not 0.0 < p < 1.0:
        raise ValueError(f"p must be strictly between 0 and 1, got {p}")
    if len(fit.coef) != 2:
        return None
    slope = fit.coef[1]
    if slope == 0:
        return None
    return (math.log(p / (1.0 - p)) - fit.coef[0]) / slope


def _quantile(sorted_values: Sequence[float], q: float) -> float:
    pos = q * (len(sorted_values) - 1)
    low = math.floor(pos)
    high = min(low + 1, len(sorted_values) - 1)
    frac = pos - low
    return sorted_values[low] * (1.0 - frac) + sorted_values[high] * frac


def bootstrap_interval[T](
    rows: Sequence[T],
    statistic: Callable[[Sequence[T]], float | None],
    *,
    cluster: Callable[[T], Hashable] | None = None,
    n: int = 1000,
    seed: int = 0,
    level: float = 0.95,
) -> tuple[float, float, int] | None:
    """Percentile bootstrap interval for ``statistic``.

    With ``cluster``, each resample draws whole clusters (rows sharing a key) with replacement,
    so rows of one task stay together. Resamples where ``statistic`` returns None are skipped.
    Returns ``(low, high, produced)``, or None when fewer than half of the resamples produced a
    value or there are no rows.
    """
    if n < 1:
        raise ValueError(f"n must be >= 1, got {n}")
    if not 0.0 < level < 1.0:
        raise ValueError(f"level must be strictly between 0 and 1, got {level}")
    if len(rows) == 0:
        return None

    groups: list[list[T]] = []
    if cluster is None:
        groups = [[row] for row in rows]
    else:
        index: dict[Hashable, list[T]] = {}
        for row in rows:
            index.setdefault(cluster(row), []).append(row)
        groups = list(index.values())

    rng = Random(seed)
    values: list[float] = []
    for _ in range(n):
        drawn = rng.choices(groups, k=len(groups))
        sample: list[T] = [row for group in drawn for row in group]
        value = statistic(sample)
        if value is not None:
            values.append(value)
    if 2 * len(values) < n:
        return None
    values.sort()
    alpha = (1.0 - level) / 2.0
    return (_quantile(values, alpha), _quantile(values, 1.0 - alpha), len(values))


def _check_probabilities(probs: Sequence[float], outcomes: Sequence[int]) -> None:
    if len(probs) == 0:
        raise ValueError("at least one prediction is required")
    if len(probs) != len(outcomes):
        raise ValueError(f"{len(probs)} probabilities but {len(outcomes)} outcomes")
    if any(not 0.0 <= p <= 1.0 for p in probs):
        raise ValueError("probabilities must be in [0, 1]")
    if any(o not in (0, 1) for o in outcomes):
        raise ValueError("outcomes must be 0 or 1")


def brier(probs: Sequence[float], outcomes: Sequence[int]) -> float:
    """Mean squared error between predicted probabilities and 0/1 outcomes."""
    _check_probabilities(probs, outcomes)
    return sum((p - o) ** 2 for p, o in zip(probs, outcomes, strict=True)) / len(probs)


@dataclass(frozen=True)
class ReliabilityBin:
    """One equal-width probability bin: its count, mean prediction and observed success rate."""

    lo: float
    hi: float
    n: int
    mean_predicted: float | None
    observed: float | None


def reliability(probs: Sequence[float], outcomes: Sequence[int], bins: int = 10) -> list[ReliabilityBin]:
    """Calibration table over ``bins`` equal-width bins on [0, 1]. A probability of 1.0 is in the last bin."""
    _check_probabilities(probs, outcomes)
    if bins < 1:
        raise ValueError(f"bins must be >= 1, got {bins}")
    buckets: list[list[tuple[float, int]]] = [[] for _ in range(bins)]
    for p, o in zip(probs, outcomes, strict=True):
        buckets[min(int(p * bins), bins - 1)].append((p, o))
    result: list[ReliabilityBin] = []
    for i, bucket in enumerate(buckets):
        count = len(bucket)
        result.append(
            ReliabilityBin(
                lo=i / bins,
                hi=(i + 1) / bins,
                n=count,
                mean_predicted=sum(p for p, _ in bucket) / count if count else None,
                observed=sum(o for _, o in bucket) / count if count else None,
            )
        )
    return result
