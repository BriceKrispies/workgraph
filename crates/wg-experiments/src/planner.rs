//! Trial planner helpers: the pure parts of `src/workgraph/experiments/planner.py` (CORE-25, GV-05).
//!
//! The planner chooses the next trial from the protocol, the trial history and the seed. The
//! choices here are pure: the staircase histories, the seeded pick, the nearest unused task, and
//! the per-arm measurements (pass rates by band, the size fit with clustered bootstrap intervals,
//! summaries). Database reads and the trial records stay in the shell.
//!
//! Floats match Python bit for bit. `statistics.fmean` is `math.fsum(data) / n`, so [`py_fsum`]
//! ports CPython's exact summation; `statistics.median` and `2.0 ** x` keep Python's operations.

use std::collections::BTreeSet;

use rpds::{RedBlackTreeMapSync, Vector};
use wg_pyrand::Random;

use crate::stats::{
    BootstrapConfig, DEFAULT_MAX_ITER, DEFAULT_TOL, LogisticFit, Penalty, ValueError, Z_95,
    bootstrap_clusters, fit_logistic, size_at_probability, sorted_values, wilson,
};

/// Multiplier of the seed in the pick generator (`_SEED_STRIDE`).
pub const SEED_STRIDE: i128 = 1_000_003;
/// Bootstrap resamples of the size fit (`_BOOTSTRAP_RESAMPLES`).
pub const BOOTSTRAP_RESAMPLES: i64 = 1000;
/// Fewest graded trials with a size that a fit needs (`_MIN_FIT_TRIALS`).
pub const MIN_FIT_TRIALS: usize = 4;
/// Confidence level of the size-fit intervals (the Python default of `bootstrap_interval`).
const INTERVAL_LEVEL: f64 = 0.95;

/// `math.fsum`: the correctly rounded sum of the values (Shewchuk's algorithm, as CPython).
///
/// Non-finite inputs fall back to the plain sum, which gives the same infinities and NaNs.
#[must_use]
pub fn py_fsum(values: &[f64]) -> f64 {
    if values.iter().any(|v| !v.is_finite()) {
        return values.iter().fold(0.0, |total, v| total + v);
    }
    let partials = values.iter().fold(Vector::<f64>::new(), |partials, &x| {
        add_partial(&partials, x)
    });
    let ascending: Vec<f64> = partials.iter().copied().collect();
    sum_exact(&ascending)
}

/// Adds `x` to the non-overlapping partials, keeping their order and dropping zero residues.
fn add_partial(partials: &Vector<f64>, x: f64) -> Vector<f64> {
    let (kept, last) = partials.iter().fold((Vector::new(), x), |(kept, x), &y| {
        let (big, small) = if x.abs() < y.abs() { (y, x) } else { (x, y) };
        let hi = big + small;
        let lo = small - (hi - big);
        let kept = if lo != 0.0 { kept.push_back(lo) } else { kept };
        (kept, hi)
    });
    if last != 0.0 {
        kept.push_back(last)
    } else {
        kept
    }
}

/// Descends the partials from the top while the running sum stays inexact, then rounds the last
/// step half-to-even across the remaining partial, as CPython's `math_fsum` does.
fn sum_exact(partials: &[f64]) -> f64 {
    match partials.split_last() {
        None => 0.0,
        Some((&top, rest)) => {
            let (hi, lo, remaining) = descend(rest, top, 0.0);
            let needs_round = remaining > 0
                && partials
                    .get(remaining - 1)
                    .is_some_and(|next| (lo < 0.0 && *next < 0.0) || (lo > 0.0 && *next > 0.0));
            if needs_round {
                let doubled = lo * 2.0;
                let x = hi + doubled;
                let yr = x - hi;
                if doubled == yr { x } else { hi }
            } else {
                hi
            }
        }
    }
}

/// Adds the partials from the top down until one leaves a non-zero low part.
/// Returns the high part, that low part, and how many partials are still unused.
fn descend(rest: &[f64], hi: f64, lo: f64) -> (f64, f64, usize) {
    match rest.split_last() {
        None => (hi, lo, 0),
        Some((&y, below)) => {
            let x = hi;
            let sum = x + y;
            let yr = sum - x;
            let residue = y - yr;
            if residue != 0.0 {
                (sum, residue, below.len())
            } else {
                descend(below, sum, residue)
            }
        }
    }
}

/// `statistics.median` and `statistics.fmean` of a sample (both `None` when empty).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    /// Median of the sorted values; the mean of the two middle values for an even count.
    pub median: Option<f64>,
    /// `math.fsum(values) / n`.
    pub mean: Option<f64>,
}

/// The median and mean of `values` (`_summary`).
#[must_use]
pub fn summary(values: &[f64]) -> Summary {
    let sorted = sorted_values(values);
    let n = sorted.len();
    if n == 0 {
        return Summary {
            median: None,
            mean: None,
        };
    }
    let half = n / 2;
    let median = if n % 2 == 1 {
        sorted.get(half).copied()
    } else {
        match (sorted.get(half - 1), sorted.get(half)) {
            (Some(low), Some(high)) => Some((low + high) / 2.0),
            _ => None,
        }
    };
    Summary {
        median,
        mean: Some(py_fsum(values) / n as f64),
    }
}

/// `dict(sorted(Counter(values).items()))` as ordered `(value, count)` pairs (`_sorted_counts`).
#[must_use]
pub fn sorted_counts(values: &[&str]) -> Vec<(String, usize)> {
    let counts = values.iter().fold(
        RedBlackTreeMapSync::<String, usize>::new_sync(),
        |counts, value| {
            let seen = counts.get(*value).copied().unwrap_or(0);
            counts.insert(value.to_string(), seen + 1)
        },
    );
    counts
        .iter()
        .map(|(value, count)| (value.clone(), *count))
        .collect()
}

/// Pass rate and Wilson interval of one band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandRow {
    /// The band.
    pub band: i64,
    /// Graded trials at the band.
    pub n: usize,
    /// Passing trials at the band.
    pub passes: usize,
    /// `passes / n`.
    pub rate: f64,
    /// The 95 % Wilson interval of the pass rate.
    pub wilson: (f64, f64),
}

/// Pass rates by band of `(band, passed)` graded trials, bands ascending (`_band_rows`).
/// Trials without a band are ignored.
///
/// # Errors
///
/// Returns a [`ValueError`] only if the Wilson interval rejects a count, which cannot happen for
/// counts taken from the sample.
pub fn band_rows(graded: &[(Option<i64>, bool)]) -> Result<Vec<BandRow>, ValueError> {
    let bands: BTreeSet<i64> = graded.iter().filter_map(|(band, _)| *band).collect();
    bands
        .into_iter()
        .map(|band| {
            let outcomes: Vec<bool> = graded
                .iter()
                .filter(|(b, _)| *b == Some(band))
                .map(|(_, passed)| *passed)
                .collect();
            let n = outcomes.len();
            let passes = outcomes.iter().filter(|passed| **passed).count();
            let (lo, hi) = wilson(
                i64::try_from(passes).unwrap_or(0),
                i64::try_from(n).unwrap_or(0),
                Z_95,
            )?;
            Ok(BandRow {
                band,
                n,
                passes,
                rate: passes as f64 / n as f64,
                wilson: (lo, hi),
            })
        })
        .collect()
}

/// `2 ** x` for a log2 size, `None` for `None` (`_lines`).
#[must_use]
pub fn lines(x: Option<f64>) -> Option<f64> {
    // `black_box` keeps the base opaque: with a literal 2.0 the optimiser rewrites `pow` as
    // `exp2`, whose rounding differs from the C `pow` that Python's `2.0 ** x` calls.
    x.map(|value| std::hint::black_box(2.0_f64).powf(value))
}

/// One graded trial of the size fit: its task, whether it passed, and its `src_lines` when the
/// replay recorded one.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeTrial {
    /// The task the trial ran.
    pub task_id: String,
    /// Whether the trial passed.
    pub passed: bool,
    /// The replay's `src_lines`, if it is a number.
    pub src_lines: Option<f64>,
}

/// A row of the size fit: `log2(src_lines)`, the outcome as 0 or 1, and the task that clusters it.
#[derive(Clone, Debug, PartialEq)]
pub struct FitRow {
    /// The task the row belongs to (the bootstrap cluster).
    pub task_id: String,
    /// `log2(src_lines)`.
    pub x: f64,
    /// 1 for a pass, 0 for a fail.
    pub y: i64,
}

/// A logistic size fit with its 50 % and 80 % sizes and, when the slope is negative, clustered
/// bootstrap intervals of those sizes in lines.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeFit {
    /// The intercept of `log2(src_lines)`.
    pub intercept: f64,
    /// The slope of `log2(src_lines)`.
    pub slope: f64,
    /// Whether the Newton iteration converged.
    pub converged: bool,
    /// Rows fitted.
    pub n: usize,
    /// `log2` size at 50 % pass probability.
    pub x50: Option<f64>,
    /// `log2` size at 80 % pass probability.
    pub x80: Option<f64>,
    /// Size in lines at 50 %.
    pub lines50: Option<f64>,
    /// Size in lines at 80 %.
    pub lines80: Option<f64>,
    /// Bootstrap interval of the 50 % size in lines.
    pub ci50: Option<(f64, f64)>,
    /// Bootstrap interval of the 80 % size in lines.
    pub ci80: Option<(f64, f64)>,
}

/// The outcome of [`size_fit`]: a fit, or the reason no fit was made.
#[derive(Clone, Debug, PartialEq)]
pub enum SizeFitOutcome {
    /// A fit was made.
    Fitted(SizeFit),
    /// No fit: the message is the Python reason.
    Rejected(String),
}

/// Refit of a bootstrap sample, returning its size in lines at probability `p` (`_threshold_statistic`).
fn threshold_statistic(sample: &[&FitRow], p: f64) -> Option<f64> {
    let xs: Vec<Vec<f64>> = sample.iter().map(|row| vec![row.x]).collect();
    let ys: Vec<i64> = sample.iter().map(|row| row.y).collect();
    let refit = fit_logistic(
        &xs,
        &ys,
        &Penalty::Uniform(0.0),
        DEFAULT_MAX_ITER,
        DEFAULT_TOL,
    )
    .ok()?;
    let slope = refit.coef.get(1).copied()?;
    if !refit.converged || slope >= 0.0 {
        return None;
    }
    size_at_probability(&refit, p).ok().flatten()
}

/// The percentile bootstrap interval of the size at `p`, in lines, resampling whole tasks
/// (`_lines_interval`).
///
/// # Errors
///
/// Returns a [`ValueError`] from the bootstrap settings, which are fixed here, so only an
/// invalid `p` could raise it.
pub fn lines_interval(
    rows: &[FitRow],
    p: f64,
    seed: i128,
) -> Result<Option<(f64, f64)>, ValueError> {
    let config = BootstrapConfig {
        n: BOOTSTRAP_RESAMPLES,
        seed,
        level: INTERVAL_LEVEL,
    };
    let interval = bootstrap_clusters(
        rows,
        |row| row.task_id.clone(),
        |sample| threshold_statistic(sample, p),
        &config,
    )?;
    Ok(interval.and_then(|(low, high, _)| Some((lines(Some(low))?, lines(Some(high))?))))
}

/// Logistic fit of pass on `log2(src_lines)`, with 50 % and 80 % sizes and task-clustered
/// intervals (`_size_fit`). Trials without a positive `src_lines` are not counted.
///
/// # Errors
///
/// Returns a [`ValueError`] only if a fit or size computation rejects its input, which the
/// checks above prevent.
pub fn size_fit(trials: &[SizeTrial], seed: i128) -> Result<SizeFitOutcome, ValueError> {
    let rows: Vec<FitRow> = trials
        .iter()
        .filter_map(|trial| {
            trial.src_lines.filter(|src| *src > 0.0).map(|src| FitRow {
                task_id: trial.task_id.clone(),
                x: src.log2(),
                y: i64::from(trial.passed),
            })
        })
        .collect();
    if rows.len() < MIN_FIT_TRIALS {
        return Ok(SizeFitOutcome::Rejected(format!(
            "needs at least {MIN_FIT_TRIALS} graded trials with src_lines, has {}",
            rows.len()
        )));
    }
    let distinct: BTreeSet<i64> = rows.iter().map(|row| row.y).collect();
    if distinct.len() < 2 {
        return Ok(SizeFitOutcome::Rejected(
            "needs both pass and fail outcomes".to_string(),
        ));
    }
    let xs: Vec<Vec<f64>> = rows.iter().map(|row| vec![row.x]).collect();
    let ys: Vec<i64> = rows.iter().map(|row| row.y).collect();
    let fit: LogisticFit = fit_logistic(
        &xs,
        &ys,
        &Penalty::Uniform(0.0),
        DEFAULT_MAX_ITER,
        DEFAULT_TOL,
    )?;
    let (intercept, slope) = match fit.coef.as_slice() {
        [intercept, slope] => (*intercept, *slope),
        _ => {
            return Ok(SizeFitOutcome::Rejected(
                "the fit has one slope".to_string(),
            ));
        }
    };
    let x50 = size_at_probability(&fit, 0.5)?;
    let x80 = size_at_probability(&fit, 0.8)?;
    let stable = fit.converged && slope < 0.0;
    let ci50 = if stable {
        lines_interval(&rows, 0.5, seed)?
    } else {
        None
    };
    let ci80 = if stable {
        lines_interval(&rows, 0.8, seed)?
    } else {
        None
    };
    Ok(SizeFitOutcome::Fitted(SizeFit {
        intercept,
        slope,
        converged: fit.converged,
        n: rows.len(),
        x50,
        x80,
        lines50: lines(x50),
        lines80: lines(x80),
        ci50,
        ci80,
    }))
}

/// A replay task the planner may draw: its key and integer band.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The task key.
    pub key: String,
    /// The task's band.
    pub band: i64,
}

/// Deterministic choice: the same seed, history position and candidates give the same task.
/// Candidates are ordered by key (ties keep their input order) before the seeded `choice`.
#[must_use]
pub fn pick(seed: i128, next_seq: i128, tasks: &[Task]) -> Option<&Task> {
    let ordered: Vec<&Task> = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| ((task.key.as_str(), index), task))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    Random::new(seed * SEED_STRIDE + next_seq)
        .choice(&ordered)
        .map(|(task, _)| *task)
}

/// The unused tasks of the band nearest `band` (ties to the lower band), in pool order
/// (`_nearest_unused`). `None` when every task is used.
#[must_use]
pub fn nearest_unused<'a>(
    pool: &'a [Task],
    used: &BTreeSet<String>,
    band: i64,
) -> Option<(i64, Vec<&'a Task>)> {
    let unused: Vec<&Task> = pool
        .iter()
        .filter(|task| !used.contains(&task.key))
        .collect();
    let chosen = unused
        .iter()
        .map(|task| task.band)
        .min_by_key(|b| ((b - band).abs(), *b))?;
    Some((
        chosen,
        unused
            .into_iter()
            .filter(|task| task.band == chosen)
            .collect(),
    ))
}

/// One trial of the primary arm as the staircase sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialFact {
    /// The arm the trial ran in.
    pub arm: String,
    /// Whether the trial is graded (not planned, running or invalid).
    pub graded: bool,
    /// The phase name the trial was planned in.
    pub phase: String,
    /// The band the staircase asked for, if recorded; otherwise the trial's own band.
    pub band: Option<i64>,
    /// Whether the outcome is a pass.
    pub passed: bool,
}

/// An ordered `(band, passed)` history of one phase.
pub type History = Vec<(i64, bool)>;

/// The staircase and refine histories of the primary arm's graded trials (`_step`'s inputs).
#[must_use]
pub fn staircase_histories(primary: &str, trials: &[TrialFact]) -> (History, History) {
    let history = |phase: &str| -> History {
        trials
            .iter()
            .filter(|t| t.arm == primary && t.graded && t.phase == phase)
            .filter_map(|t| t.band.map(|band| (band, t.passed)))
            .collect()
    };
    (history("staircase"), history("refine"))
}
