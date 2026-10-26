//! Success predictor: the pure parts of `src/workgraph/experiments/predictor.py` (CORE-24).
//!
//! The features are the size a leaf is predicted to have (log2 source lines, log2 files) and the
//! number of packages it touches. Each repository gets its own intercept, shrunk toward the
//! shared fit by an L2 penalty. Database reads and the fit's storage stay in the shell; this
//! module holds the arithmetic and the decisions around it.

use crate::stats::{ValueError, py_sum, quantile, sigmoid, sorted_values};
use std::iter::{once, repeat_n};

/// Fewest graded trials a fit needs (`MIN_ROWS`).
pub const MIN_ROWS: usize = 4;
/// Fewest bootstrap draws that give a prediction interval (`MIN_SAMPLES_FOR_INTERVAL`).
pub const MIN_SAMPLES_FOR_INTERVAL: usize = 20;
/// Names of the feature columns, in order (`FEATURE_NAMES`).
pub const FEATURE_NAMES: [&str; 3] = ["log2_src_lines", "log2_src_files", "packages"];

/// Why a set of graded trials cannot be fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitRejection {
    /// Fewer than [`MIN_ROWS`] rows.
    TooFewRows {
        /// Rows available.
        rows: usize,
    },
    /// Every row passed, so there is nothing to fit against.
    OnlyPasses,
    /// Every row failed, so there is nothing to fit against.
    OnlyFails,
}

/// The ex-ante feature vector: log2 lines and files (each at least 1), then the package count.
#[must_use]
pub fn features_of(src_lines: f64, src_files: f64, packages: usize) -> [f64; 3] {
    [
        at_least_one(src_lines).log2(),
        at_least_one(src_files).log2(),
        packages as f64,
    ]
}

/// Python's `max(1.0, value)`.
fn at_least_one(value: f64) -> f64 {
    if value > 1.0 { value } else { 1.0 }
}

/// Predicted probability for one feature vector. `coef` is intercept first; `x` holds the
/// features without an intercept column.
///
/// # Errors
///
/// Returns a [`ValueError`] when `coef` is empty or does not have one slope per feature.
pub fn probability(coef: &[f64], x: &[f64]) -> Result<f64, ValueError> {
    let Some((intercept, slopes)) = coef.split_first() else {
        return Err(ValueError::new("a coefficient vector needs an intercept"));
    };
    if slopes.len() != x.len() {
        return Err(ValueError::new(format!(
            "coef has {} slopes but x has {} features",
            slopes.len(),
            x.len()
        )));
    }
    Ok(sigmoid(
        intercept + py_sum(slopes.iter().zip(x).map(|(c, v)| c * v)),
    ))
}

/// Penalty per column of the design matrix: the intercept is free, then the three feature
/// columns with `feature_penalty`, then one column per repository with `repo_penalty`.
#[must_use]
pub fn penalty_vector(repositories: usize, feature_penalty: f64, repo_penalty: f64) -> Vec<f64> {
    once(0.0)
        .chain(repeat_n(feature_penalty, FEATURE_NAMES.len()))
        .chain(repeat_n(repo_penalty, repositories))
        .collect()
}

/// Design matrix: each row is its features followed by one 0/1 indicator per repository in
/// `repositories`. A row without a repository (or with one outside the list) gets no indicator.
#[must_use]
pub fn design_matrix<K: PartialEq>(
    rows: &[(Vec<f64>, Option<K>)],
    repositories: &[K],
) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|(features, repository)| {
            features
                .iter()
                .copied()
                .chain(repositories.iter().map(|repo| {
                    if repository.as_ref() == Some(repo) {
                        1.0
                    } else {
                        0.0
                    }
                }))
                .collect()
        })
        .collect()
}

/// The 95 % prediction interval from bootstrap coefficient draws, or `None` with fewer than
/// [`MIN_SAMPLES_FOR_INTERVAL`] draws.
///
/// # Errors
///
/// Returns a [`ValueError`] when a draw does not match the feature vector `x`.
pub fn prediction_interval(
    draws: &[Vec<f64>],
    x: &[f64],
) -> Result<Option<(f64, f64)>, ValueError> {
    if draws.len() < MIN_SAMPLES_FOR_INTERVAL {
        return Ok(None);
    }
    let values = draws
        .iter()
        .map(|draw| probability(draw, x))
        .collect::<Result<Vec<f64>, ValueError>>()?;
    let sorted = sorted_values(&values);
    Ok(quantile(&sorted, 0.025).zip(quantile(&sorted, 0.975)))
}

/// Decides whether `rows` graded trials with `passes` passing ones can be fitted.
///
/// # Errors
///
/// Returns the [`FitRejection`] that applies, checked in the order Python checks it.
pub fn check_fit_rows(rows: usize, passes: usize) -> Result<(), FitRejection> {
    if rows < MIN_ROWS {
        Err(FitRejection::TooFewRows { rows })
    } else if passes == 0 {
        Err(FitRejection::OnlyFails)
    } else if passes == rows {
        Err(FitRejection::OnlyPasses)
    } else {
        Ok(())
    }
}

/// Validates the penalties and the bootstrap sample count of a fit request.
///
/// # Errors
///
/// Returns a [`ValueError`] for a negative penalty or a negative `samples`.
pub fn check_fit_settings(
    repo_penalty: f64,
    feature_penalty: f64,
    samples: i64,
) -> Result<(), ValueError> {
    if repo_penalty < 0.0 || feature_penalty < 0.0 {
        return Err(ValueError::new("penalties must be >= 0"));
    }
    if samples < 0 {
        return Err(ValueError::new(format!(
            "samples must be >= 0, got {samples}"
        )));
    }
    Ok(())
}

/// Validates the size inputs of a prediction.
///
/// # Errors
///
/// Returns a [`ValueError`] when either size is negative.
pub fn check_prediction_sizes(src_lines: f64, src_files: f64) -> Result<(), ValueError> {
    if src_lines < 0.0 || src_files < 0.0 {
        return Err(ValueError::new("src_lines and src_files must be >= 0"));
    }
    Ok(())
}
