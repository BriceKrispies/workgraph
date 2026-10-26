//! Standard-library statistics for capability experiments (CORE-24, GV-05).
//!
//! Port of `src/workgraph/experiments/stats.py`: Wilson intervals for pass rates, penalised
//! logistic regression fitted by Newton-Raphson, percentile bootstrap intervals (optionally
//! resampling whole clusters, drawn with [`wg_pyrand::Random`] so a seed gives CPython's
//! resamples), and calibration measures.
//!
//! Floating-point results equal the Python results bit for bit: every expression keeps the
//! operand order of the Python source, `sum()` over floats is CPython 3.12's compensated
//! summation ([`py_sum`]), and `max`/`min` keep the first argument on ties as Python does.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::hash::Hash;
use std::iter::{once, repeat_n};
use std::ops::ControlFlow;

use rpds::RedBlackTreeMapSync;
use wg_pyrand::Random;

/// `scipy`-free 95 % normal quantile used by the Python module (`_Z_95`).
pub const Z_95: f64 = 1.959_963_984_540_054;
/// Default `tol` of [`fit_logistic`].
pub const DEFAULT_TOL: f64 = 1e-9;
/// Default `max_iter` of [`fit_logistic`].
pub const DEFAULT_MAX_ITER: i64 = 100;

const STEP_HALVINGS: usize = 30;
const SLACK: f64 = 1e-10;

/// A Python `ValueError`: the message is the one the Python function raises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueError(String);

impl ValueError {
    /// Creates an error carrying `message`.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The message, identical to the Python `str(exc)`.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

fn invalid<T>(message: String) -> Result<T, ValueError> {
    Err(ValueError(message))
}

/// CPython 3.12+ `sum()` over floats, shared with the other Core crates in `wg-pynum`.
pub use wg_pynum::py_sum;

/// Python's `max(a, b)` for floats: `b` only when strictly greater than `a`.
fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)` for floats: `b` only when strictly less than `a`.
fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Wilson score interval for a binomial proportion. `(0.0, 1.0)` when `n` is 0.
///
/// # Errors
///
/// Returns a [`ValueError`] when `n < 0` or `successes` is outside `[0, n]`.
pub fn wilson(successes: i64, n: i64, z: f64) -> Result<(f64, f64), ValueError> {
    if n < 0 {
        return invalid(format!("n must be >= 0, got {n}"));
    }
    if successes < 0 || successes > n {
        return invalid(format!("successes must be in [0, {n}], got {successes}"));
    }
    if n == 0 {
        return Ok((0.0, 1.0));
    }
    let nf = n as f64;
    let p_hat = successes as f64 / nf;
    let z2 = z * z;
    let denom = 1.0 + z2 / nf;
    let center = (p_hat + z2 / (2.0 * nf)) / denom;
    let half = z * (p_hat * (1.0 - p_hat) / nf + z2 / (4.0 * nf * nf)).sqrt() / denom;
    Ok((py_max(0.0, center - half), py_min(1.0, center + half)))
}

/// A fitted logistic model. `coef[0]` is the intercept; `log_likelihood` is unpenalised.
#[derive(Clone, Debug, PartialEq)]
pub struct LogisticFit {
    /// Intercept first, then one coefficient per feature.
    pub coef: Vec<f64>,
    /// Number of rows fitted.
    pub n: usize,
    /// Whether the Newton iteration reached the step tolerance.
    pub converged: bool,
    /// Newton steps taken (accepted or refused).
    pub iterations: usize,
    /// Unpenalised log-likelihood at the returned coefficients.
    pub log_likelihood: f64,
    /// The penalty applied to each coefficient, intercept first.
    pub penalty: Vec<f64>,
}

/// The `penalty` argument of [`fit_logistic`]: one value for every feature coefficient (the
/// intercept is unpenalised), or one value per coefficient with the intercept first.
#[derive(Clone, Debug, PartialEq)]
pub enum Penalty {
    /// A single float, applied to every feature coefficient.
    Uniform(f64),
    /// One value per coefficient, intercept first.
    PerCoefficient(Vec<f64>),
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    py_sum(a.iter().zip(b).map(|(x, y)| x * y))
}

/// The logistic function, evaluated without overflow for large negative arguments.
#[must_use]
pub fn sigmoid(eta: f64) -> f64 {
    if eta >= 0.0 {
        1.0 / (1.0 + (-eta).exp())
    } else {
        let e = eta.exp();
        e / (1.0 + e)
    }
}

/// `log(1 + exp(eta))` without overflow.
fn log1pexp(eta: f64) -> f64 {
    py_max(eta, 0.0) + (-eta.abs()).exp().ln_1p()
}

fn log_likelihood(rows: &[Vec<f64>], targets: &[f64], beta: &[f64]) -> f64 {
    rows.iter().zip(targets).fold(0.0_f64, |total, (row, &yi)| {
        let eta = dot(row, beta);
        total + (yi * eta - log1pexp(eta))
    })
}

/// Solves `matrix @ x = rhs` by Gaussian elimination with partial pivoting; `None` if singular.
fn solve(matrix: Vec<Vec<f64>>, rhs: &[f64]) -> Option<Vec<f64>> {
    let size = rhs.len();
    let augmented: Vec<Vec<f64>> = matrix
        .into_iter()
        .zip(rhs)
        .map(|(row, r)| row.into_iter().chain(once(*r)).collect())
        .collect();
    let triangular = (0..size).try_fold(augmented, |a, col| eliminate_column(a, col, size))?;
    Some(back_substitute(&triangular, size))
}

fn eliminate_column(a: Vec<Vec<f64>>, col: usize, size: usize) -> Option<Vec<Vec<f64>>> {
    // `max(range(col, size), key=abs)`: the first row wins ties.
    let pivot = (col..size).fold(col, |best, r| {
        if a[r][col].abs() > a[best][col].abs() {
            r
        } else {
            best
        }
    });
    if a[pivot][col].abs() <= 1e-300 {
        return None;
    }
    let swapped: Vec<Vec<f64>> = a
        .iter()
        .enumerate()
        .map(|(i, row)| {
            if i == col {
                a[pivot].clone()
            } else if i == pivot {
                a[col].clone()
            } else {
                row.clone()
            }
        })
        .collect();
    let pivot_row = swapped[col].clone();
    let pivot_value = pivot_row[col];
    Some(
        swapped
            .iter()
            .enumerate()
            .map(|(r, row)| {
                if r <= col {
                    return row.clone();
                }
                let factor = row[col] / pivot_value;
                if factor == 0.0 {
                    return row.clone();
                }
                row.iter()
                    .enumerate()
                    .map(|(c, v)| {
                        if c >= col {
                            v - factor * pivot_row[c]
                        } else {
                            *v
                        }
                    })
                    .collect()
            })
            .collect(),
    )
}

fn back_substitute(a: &[Vec<f64>], size: usize) -> Vec<f64> {
    // `tail` holds x[r + 1..size] in ascending order while r walks upward from the bottom row.
    (0..size).rev().fold(Vec::with_capacity(size), |tail, r| {
        let row = &a[r];
        let sum = py_sum(row[r + 1..size].iter().zip(&tail).map(|(c, x)| c * x));
        let value = (row[size] - sum) / row[r];
        once(value).chain(tail).collect()
    })
}

/// Gradient and Hessian of the penalised log-likelihood at `beta`, accumulated row by row.
fn gradient_and_hessian(
    rows: &[Vec<f64>],
    targets: &[f64],
    pen: &[f64],
    beta: &[f64],
) -> (Vec<f64>, Vec<Vec<f64>>) {
    let size = beta.len();
    let grad0: Vec<f64> = pen.iter().zip(beta).map(|(p, b)| -p * b).collect();
    let hess0: Vec<Vec<f64>> = repeat_n(repeat_n(0.0_f64, size).collect(), size).collect();
    let (grad, hess) = rows.iter().zip(targets).fold(
        (grad0, hess0),
        |(grad, hess): (Vec<f64>, Vec<Vec<f64>>), (row, &yi)| {
            let mu = sigmoid(dot(row, beta));
            let weight = mu * (1.0 - mu);
            let residual = yi - mu;
            let grad: Vec<f64> = grad
                .iter()
                .zip(row)
                .map(|(g, rj)| g + residual * rj)
                .collect();
            let hess: Vec<Vec<f64>> = hess
                .iter()
                .zip(row)
                .map(|(hrow, rj)| {
                    hrow.iter()
                        .zip(row)
                        .map(|(h, rk)| h + weight * rj * rk)
                        .collect()
                })
                .collect();
            (grad, hess)
        },
    );
    let hess: Vec<Vec<f64>> = hess
        .iter()
        .enumerate()
        .map(|(j, hrow)| {
            hrow.iter()
                .enumerate()
                .map(|(k, h)| if j == k { h + pen[j] } else { *h })
                .collect()
        })
        .collect();
    (grad, hess)
}

/// Step halving: the first `scale = 2^-k` whose candidate does not lower the objective (up to
/// rounding) is accepted. Returns the candidate and its objective value.
fn halve(
    beta: &[f64],
    step: &[f64],
    current: f64,
    objective: &impl Fn(&[f64]) -> f64,
) -> Option<(Vec<f64>, f64)> {
    let floor = current - SLACK * (1.0 + current.abs());
    (0..STEP_HALVINGS).find_map(|k| {
        let scale = 0.5_f64.powi(i32::try_from(k).unwrap_or(0));
        let candidate: Vec<f64> = beta.iter().zip(step).map(|(b, s)| b + scale * s).collect();
        let value = objective(&candidate);
        (value >= floor).then_some((candidate, value))
    })
}

#[derive(Clone, Debug)]
struct Newton {
    beta: Vec<f64>,
    current: f64,
    iterations: usize,
}

/// One Newton iteration. `Break` carries the final state and whether it converged.
fn newton_step(
    rows: &[Vec<f64>],
    targets: &[f64],
    pen: &[f64],
    objective: &impl Fn(&[f64]) -> f64,
    tol: f64,
    state: Newton,
) -> ControlFlow<(Newton, bool), Newton> {
    let (grad, hess) = gradient_and_hessian(rows, targets, pen, &state.beta);
    let Some(step) = solve(hess, &grad) else {
        return ControlFlow::Break((state, false));
    };
    let iterations = state.iterations + 1;
    let largest = step
        .iter()
        .map(|s| s.abs())
        .reduce(|best, v| if v > best { v } else { best });
    if largest.is_some_and(|m| m <= tol) {
        let beta = state.beta.iter().zip(&step).map(|(b, s)| b + s).collect();
        return ControlFlow::Break((
            Newton {
                beta,
                current: state.current,
                iterations,
            },
            true,
        ));
    }
    match halve(&state.beta, &step, state.current, objective) {
        Some((beta, current)) => ControlFlow::Continue(Newton {
            beta,
            current,
            iterations,
        }),
        None => ControlFlow::Break((
            Newton {
                beta: state.beta,
                current: state.current,
                iterations,
            },
            false,
        )),
    }
}

/// Fits `P(y=1) = sigmoid(b0 + b . x)` by penalised maximum likelihood.
///
/// The objective is `sum(log lik) - 0.5 * sum(penalty_j * coef_j**2)`. Separable data with no
/// penalty does not converge: the fit stops at `max_iter` with `converged == false`.
///
/// # Errors
///
/// Returns a [`ValueError`] for empty or ragged input, a length mismatch between `x` and `y`,
/// `max_iter < 1`, non-finite features, labels other than 0 or 1, a penalty of the wrong
/// length, or a negative penalty.
pub fn fit_logistic(
    x: &[Vec<f64>],
    y: &[i64],
    penalty: &Penalty,
    max_iter: i64,
    tol: f64,
) -> Result<LogisticFit, ValueError> {
    if x.is_empty() {
        return invalid("fit_logistic needs at least one row".to_string());
    }
    if x.len() != y.len() {
        return invalid(format!(
            "x has {} rows but y has {} values",
            x.len(),
            y.len()
        ));
    }
    if max_iter < 1 {
        return invalid(format!("max_iter must be >= 1, got {max_iter}"));
    }
    let width = x.first().map_or(0, Vec::len);
    if x.iter().any(|row| row.len() != width) {
        return invalid("all rows of x must have the same number of features".to_string());
    }
    if x.iter().flatten().any(|v| !v.is_finite()) {
        return invalid("feature values must be finite".to_string());
    }
    if y.iter().any(|yi| *yi != 0 && *yi != 1) {
        return invalid("y values must be 0 or 1".to_string());
    }
    let size = width + 1;
    let pen: Vec<f64> = match penalty {
        Penalty::Uniform(value) => once(0.0).chain(repeat_n(*value, width)).collect(),
        Penalty::PerCoefficient(values) => {
            if values.len() != size {
                return invalid(format!(
                    "penalty needs {size} values (intercept first), got {}",
                    values.len()
                ));
            }
            values.clone()
        }
    };
    if pen.iter().any(|v| *v < 0.0) {
        return invalid("penalty values must be >= 0".to_string());
    }

    let rows: Vec<Vec<f64>> = x
        .iter()
        .map(|row| once(1.0).chain(row.iter().copied()).collect())
        .collect();
    let targets: Vec<f64> = y.iter().map(|&yi| yi as f64).collect();
    let objective = |beta: &[f64]| -> f64 {
        let penalty_term = 0.5 * py_sum(pen.iter().zip(beta).map(|(p, b)| p * b * b));
        log_likelihood(&rows, &targets, beta) - penalty_term
    };

    let zeros: Vec<f64> = repeat_n(0.0_f64, size).collect();
    let start = Newton {
        current: objective(&zeros),
        beta: zeros,
        iterations: 0,
    };
    let outcome = (0..max_iter).try_fold(start, |state, _| {
        newton_step(&rows, &targets, &pen, &objective, tol, state)
    });
    let (fitted, converged) = match outcome {
        ControlFlow::Continue(state) => (state, false),
        ControlFlow::Break(done) => done,
    };

    Ok(LogisticFit {
        log_likelihood: log_likelihood(&rows, &targets, &fitted.beta),
        coef: fitted.beta,
        n: y.len(),
        converged,
        iterations: fitted.iterations,
        penalty: pen,
    })
}

/// Predicted probability of success for one feature vector (intercept excluded).
///
/// # Errors
///
/// Returns a [`ValueError`] when `x` does not have one value per feature of `fit`.
pub fn predict(fit: &LogisticFit, x: &[f64]) -> Result<f64, ValueError> {
    let Some((intercept, slopes)) = fit.coef.split_first() else {
        return invalid("a fit has at least an intercept".to_string());
    };
    if x.len() != slopes.len() {
        return invalid(format!(
            "expected {} features, got {}",
            slopes.len(),
            x.len()
        ));
    }
    Ok(sigmoid(intercept + dot(slopes, x)))
}

/// The feature value where the predicted probability equals `p` (single-feature fits only).
///
/// # Errors
///
/// Returns a [`ValueError`] when `p` is not strictly between 0 and 1.
pub fn size_at_probability(fit: &LogisticFit, p: f64) -> Result<Option<f64>, ValueError> {
    if !(p > 0.0 && p < 1.0) {
        return invalid(format!("p must be strictly between 0 and 1, got {p:?}"));
    }
    let (intercept, slope) = match fit.coef.as_slice() {
        [intercept, slope] => (*intercept, *slope),
        _ => return Ok(None),
    };
    if slope == 0.0 {
        return Ok(None);
    }
    Ok(Some(((p / (1.0 - p)).ln() - intercept) / slope))
}

/// Sorts ascending. Equal values keep their input order, as Python's stable `sorted` does
/// (`-0.0` and `0.0` compare equal), so the result equals `sorted(values)` element for element.
#[must_use]
pub fn sorted_values(values: &[f64]) -> Vec<f64> {
    let tree = values
        .iter()
        .enumerate()
        .fold(RedBlackTreeMapSync::new_sync(), |tree, (i, v)| {
            tree.insert((SortKey(*v), i), ())
        });
    tree.keys().map(|key| key.0.0).collect()
}

#[derive(Clone, Copy, Debug)]
struct SortKey(f64);

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for SortKey {}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or_else(|| self.0.total_cmp(&other.0))
    }
}

/// CPython's `_percentile` / `_quantile`: linear interpolation at position `q * (n - 1)` of a
/// sorted slice. `None` for an empty slice.
#[must_use]
pub fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let pos = q * last as f64;
    let low = pos.floor();
    let low_index = low as usize;
    let high_index = (low_index + 1).min(last);
    let frac = pos - low;
    Some(sorted.get(low_index)? * (1.0 - frac) + sorted.get(high_index)? * frac)
}

/// Sampling settings of a percentile bootstrap (`n`, `seed`, `level` of the Python function).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BootstrapConfig {
    /// Number of resamples.
    pub n: i64,
    /// Seed of the resampling generator.
    pub seed: i128,
    /// Confidence level, strictly between 0 and 1.
    pub level: f64,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            n: 1000,
            seed: 0,
            level: 0.95,
        }
    }
}

/// A bootstrap interval: `(low, high, produced)` where `produced` counts the resamples that
/// returned a value.
pub type Interval = (f64, f64, usize);

impl BootstrapConfig {
    fn validate(&self) -> Result<(), ValueError> {
        if self.n < 1 {
            return invalid(format!("n must be >= 1, got {}", self.n));
        }
        if !(self.level > 0.0 && self.level < 1.0) {
            return invalid(format!(
                "level must be strictly between 0 and 1, got {:?}",
                self.level
            ));
        }
        Ok(())
    }
}

/// Percentile bootstrap interval for `statistic`, resampling single rows.
///
/// Resamples where `statistic` returns `None` are skipped. Returns `None` when there are no
/// rows or fewer than half of the resamples produced a value.
///
/// # Errors
///
/// Returns a [`ValueError`] when `config.n < 1` or `config.level` is not strictly between 0
/// and 1.
pub fn bootstrap_interval<T>(
    rows: &[T],
    statistic: impl Fn(&[&T]) -> Option<f64>,
    config: &BootstrapConfig,
) -> Result<Option<Interval>, ValueError> {
    config.validate()?;
    if rows.is_empty() {
        return Ok(None);
    }
    let groups: Vec<Vec<&T>> = rows.iter().map(|row| vec![row]).collect();
    Ok(interval(&groups, &statistic, config))
}

/// Percentile bootstrap interval for `statistic`, where each resample draws whole clusters
/// (rows sharing a `key`) with replacement, so rows of one cluster stay together.
///
/// # Errors
///
/// Returns a [`ValueError`] when `config.n < 1` or `config.level` is not strictly between 0
/// and 1.
pub fn bootstrap_clusters<T, K: Eq + Hash>(
    rows: &[T],
    key: impl Fn(&T) -> K,
    statistic: impl Fn(&[&T]) -> Option<f64>,
    config: &BootstrapConfig,
) -> Result<Option<Interval>, ValueError> {
    config.validate()?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(interval(&cluster_groups(rows, key), &statistic, config))
}

/// Groups rows by key, groups in order of first appearance and rows in input order.
fn cluster_groups<T, K: Eq + Hash>(rows: &[T], key: impl Fn(&T) -> K) -> Vec<Vec<&T>> {
    let keys: Vec<K> = rows.iter().map(&key).collect();
    // Collecting in reverse leaves the smallest index for each key.
    let first: HashMap<&K, usize> = keys.iter().enumerate().rev().map(|(i, k)| (k, i)).collect();
    let starts: BTreeSet<usize> = first.values().copied().collect();
    starts
        .into_iter()
        .map(|start| {
            let wanted = &keys[start];
            rows.iter()
                .zip(&keys)
                .filter(|(_, k)| *k == wanted)
                .map(|(row, _)| row)
                .collect()
        })
        .collect()
}

/// One resample: draws `groups.len()` groups with CPython's `choices`, runs the statistic on
/// the flattened sample, and returns the value with the advanced generator.
fn resample<T>(
    groups: &[Vec<&T>],
    statistic: &impl Fn(&[&T]) -> Option<f64>,
    rng: Random,
) -> (Option<f64>, Random) {
    match rng.clone().choices(groups, None, None, groups.len()) {
        Ok((drawn, next)) => {
            let sample: Vec<&T> = drawn
                .into_iter()
                .flat_map(|group| group.iter().copied())
                .collect();
            (statistic(&sample), next)
        }
        Err(_) => (None, rng),
    }
}

fn interval<T>(
    groups: &[Vec<&T>],
    statistic: &impl Fn(&[&T]) -> Option<f64>,
    config: &BootstrapConfig,
) -> Option<Interval> {
    let count = usize::try_from(config.n).ok()?;
    // Exactly `count` resamples run; `Iterator::successors` would compute one resample ahead.
    let (_, produced) = (0..count).fold(
        (Random::new(config.seed), rpds::Vector::new()),
        |(rng, values): (Random, rpds::Vector<f64>), _| {
            let (value, next) = resample(groups, statistic, rng);
            let values = match value {
                Some(v) => values.push_back(v),
                None => values,
            };
            (next, values)
        },
    );
    let draws: Vec<f64> = produced.iter().copied().collect();
    if 2 * draws.len() < count {
        return None;
    }
    let sorted = sorted_values(&draws);
    let alpha = (1.0 - config.level) / 2.0;
    Some((
        quantile(&sorted, alpha)?,
        quantile(&sorted, 1.0 - alpha)?,
        sorted.len(),
    ))
}

fn check_probabilities(probs: &[f64], outcomes: &[i64]) -> Result<(), ValueError> {
    if probs.is_empty() {
        return invalid("at least one prediction is required".to_string());
    }
    if probs.len() != outcomes.len() {
        return invalid(format!(
            "{} probabilities but {} outcomes",
            probs.len(),
            outcomes.len()
        ));
    }
    if probs.iter().any(|p| !(0.0..=1.0).contains(p)) {
        return invalid("probabilities must be in [0, 1]".to_string());
    }
    if outcomes.iter().any(|o| *o != 0 && *o != 1) {
        return invalid("outcomes must be 0 or 1".to_string());
    }
    Ok(())
}

/// Mean squared error between predicted probabilities and 0/1 outcomes.
///
/// # Errors
///
/// Returns a [`ValueError`] for empty or mismatched input, probabilities outside `[0, 1]`, or
/// outcomes other than 0 or 1.
pub fn brier(probs: &[f64], outcomes: &[i64]) -> Result<f64, ValueError> {
    check_probabilities(probs, outcomes)?;
    let total = py_sum(
        probs
            .iter()
            .zip(outcomes)
            .map(|(p, &o)| (p - o as f64).powf(2.0)),
    );
    Ok(total / probs.len() as f64)
}

/// One equal-width probability bin of a calibration table.
#[derive(Clone, Debug, PartialEq)]
pub struct ReliabilityBin {
    /// Lower bound of the bin.
    pub lo: f64,
    /// Upper bound of the bin.
    pub hi: f64,
    /// Predictions in the bin.
    pub n: usize,
    /// Mean prediction in the bin, `None` when empty.
    pub mean_predicted: Option<f64>,
    /// Observed success rate in the bin, `None` when empty.
    pub observed: Option<f64>,
}

/// Calibration table over `bins` equal-width bins on `[0, 1]`. A probability of 1.0 falls in
/// the last bin.
///
/// # Errors
///
/// Returns a [`ValueError`] for invalid probabilities or outcomes, or `bins < 1`.
pub fn reliability(
    probs: &[f64],
    outcomes: &[i64],
    bins: i64,
) -> Result<Vec<ReliabilityBin>, ValueError> {
    check_probabilities(probs, outcomes)?;
    if bins < 1 {
        return invalid(format!("bins must be >= 1, got {bins}"));
    }
    let count = usize::try_from(bins).unwrap_or(1);
    let bin_of = |p: f64| ((p * bins as f64) as usize).min(count - 1);
    Ok((0..count)
        .map(|i| {
            let bucket: Vec<(f64, i64)> = probs
                .iter()
                .zip(outcomes)
                .filter(|(p, _)| bin_of(**p) == i)
                .map(|(p, o)| (*p, *o))
                .collect();
            let n = bucket.len();
            let (mean_predicted, observed) = if n == 0 {
                (None, None)
            } else {
                let mean = py_sum(bucket.iter().map(|(p, _)| *p)) / n as f64;
                let successes: i64 = bucket.iter().map(|(_, o)| *o).sum();
                (Some(mean), Some(successes as f64 / n as f64))
            };
            ReliabilityBin {
                lo: i as f64 / bins as f64,
                hi: (i + 1) as f64 / bins as f64,
                n,
                mean_predicted,
                observed,
            }
        })
        .collect())
}
