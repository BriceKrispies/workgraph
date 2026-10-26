//! Port of `tests/unit/test_experiment_stats.py`: the same cases, with the same tolerances, over
//! the Rust statistics. The generated datasets come from `wg_pyrand::Random`, which reproduces
//! CPython's generator, so the samples equal the ones the Python tests draw.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;

use wg_experiments::stats::{
    BootstrapConfig, DEFAULT_MAX_ITER, DEFAULT_TOL, LogisticFit, Penalty, ReliabilityBin, Z_95,
    bootstrap_clusters, bootstrap_interval, brier, fit_logistic, predict, reliability,
    size_at_probability, wilson,
};
use wg_pyrand::Random;

type TestResult = Result<(), Box<dyn Error>>;

/// `random.Random.random()` on a generator held in `rng`.
fn draw(rng: &mut Random) -> f64 {
    let (value, next) = rng.clone().random();
    *rng = next;
    value
}

/// `random.Random.uniform(a, b)`.
fn uniform(rng: &mut Random, a: f64, b: f64) -> f64 {
    a + (b - a) * draw(rng)
}

fn approx(actual: f64, expected: f64, abs: f64) -> bool {
    (actual - expected).abs() <= abs
}

fn staircase_sample(seed: i128, n: usize) -> (Vec<Vec<f64>>, Vec<i64>) {
    let mut rng = Random::new(seed);
    (0..n)
        .map(|_| {
            let value = uniform(&mut rng, 0.0, 12.0);
            let p = 1.0 / (1.0 + (-(6.0 - value)).exp());
            let outcome = i64::from(draw(&mut rng) < p);
            (vec![value], outcome)
        })
        .unzip()
}

fn fit(x: &[Vec<f64>], y: &[i64], penalty: Penalty) -> Result<LogisticFit, Box<dyn Error>> {
    Ok(fit_logistic(x, y, &penalty, DEFAULT_MAX_ITER, DEFAULT_TOL)?)
}

// Wilson ---------------------------------------------------------------------------------------

#[test]
fn test_wilson_zero_successes() -> TestResult {
    // With no successes the upper bound has the closed form z^2 / (n + z^2).
    let z2 = Z_95 * Z_95;
    let (lo, hi) = wilson(0, 10, Z_95)?;
    assert_eq!(lo, 0.0);
    assert!(approx(hi, z2 / (10.0 + z2), 1e-6));
    assert!(approx(hi, 0.2775328, 1e-6));
    Ok(())
}

#[test]
fn test_wilson_half_successes() -> TestResult {
    let (lo, hi) = wilson(5, 10, Z_95)?;
    assert!(approx(lo, 0.236593, 1e-6));
    assert!(approx(hi, 0.763407, 1e-6));
    Ok(())
}

#[test]
fn test_wilson_all_successes() -> TestResult {
    // With all successes the lower bound has the closed form n / (n + z^2).
    let z2 = Z_95 * Z_95;
    let (lo, hi) = wilson(10, 10, Z_95)?;
    assert!(approx(lo, 10.0 / (10.0 + z2), 1e-6));
    assert!(approx(lo, 0.7224672, 1e-6));
    assert!(approx(hi, 1.0, 1e-12));
    Ok(())
}

#[test]
fn test_wilson_empty_sample_is_uninformative() -> TestResult {
    assert_eq!(wilson(0, 0, Z_95)?, (0.0, 1.0));
    Ok(())
}

#[test]
fn test_wilson_rejects_invalid_counts() {
    for (successes, n) in [(-1, 10), (11, 10), (1, 0), (-1, 0)] {
        assert!(
            wilson(successes, n, Z_95).is_err(),
            "wilson({successes}, {n})"
        );
    }
}

// Logistic regression --------------------------------------------------------------------------

#[test]
fn test_fit_recovers_known_parameters() -> TestResult {
    let (x, y) = staircase_sample(1, 4000);
    let fitted = fit(&x, &y, Penalty::Uniform(0.0))?;
    assert!(fitted.converged);
    assert_eq!(fitted.n, 4000);
    // Tolerances are about 3 theoretical standard errors (0.20 intercept, 0.03 slope).
    assert!(approx(fitted.coef[0], 6.0, 0.6));
    assert!(approx(fitted.coef[1], -1.0, 0.1));
    let midpoint = size_at_probability(&fitted, 0.5)?.ok_or("no midpoint")?;
    assert!(approx(midpoint, 6.0, 0.3));
    Ok(())
}

#[test]
fn test_penalty_shrinks_slope_toward_zero() -> TestResult {
    let (x, y) = staircase_sample(1, 300);
    let free = fit(&x, &y, Penalty::Uniform(0.0))?;
    let shrunk = fit(&x, &y, Penalty::Uniform(200.0))?;
    assert!(free.converged && shrunk.converged);
    assert!(shrunk.coef[1].abs() < free.coef[1].abs());
    assert_eq!(shrunk.penalty, vec![0.0, 200.0]);
    assert_eq!(free.penalty, vec![0.0, 0.0]);
    Ok(())
}

#[test]
fn test_penalty_sequence_applies_per_coefficient() -> TestResult {
    let (x, y) = staircase_sample(1, 300);
    let fitted = fit(&x, &y, Penalty::PerCoefficient(vec![0.0, 500.0]))?;
    assert_eq!(fitted.penalty, vec![0.0, 500.0]);
    assert!(fitted.coef[1].abs() < fit(&x, &y, Penalty::Uniform(0.0))?.coef[1].abs());
    Ok(())
}

#[test]
fn test_penalty_sequence_must_cover_intercept() {
    let (x, y) = staircase_sample(1, 50);
    let short = fit_logistic(
        &x,
        &y,
        &Penalty::PerCoefficient(vec![1.0]),
        100,
        DEFAULT_TOL,
    );
    assert!(short.is_err_and(|e| e.message().contains("penalty")));
    let long = fit_logistic(
        &x,
        &y,
        &Penalty::PerCoefficient(vec![0.0, 1.0, 1.0]),
        100,
        DEFAULT_TOL,
    );
    assert!(long.is_err_and(|e| e.message().contains("penalty")));
}

#[test]
fn test_negative_penalty_rejected() {
    let (x, y) = staircase_sample(1, 50);
    assert!(fit_logistic(&x, &y, &Penalty::Uniform(-1.0), 100, DEFAULT_TOL).is_err());
}

#[test]
fn test_separable_data_without_penalty_does_not_converge() -> TestResult {
    let x = vec![vec![0.0], vec![1.0], vec![2.0], vec![3.0]];
    let y = [0, 0, 1, 1];
    let fitted = fit_logistic(&x, &y, &Penalty::Uniform(0.0), 50, DEFAULT_TOL)?;
    assert!(!fitted.converged);
    assert!(fitted.iterations <= 50);
    // The slope keeps growing: no finite maximum exists.
    assert!(fitted.coef[1] > 10.0);
    Ok(())
}

#[test]
fn test_separable_data_with_penalty_converges() -> TestResult {
    let x = vec![vec![0.0], vec![1.0], vec![2.0], vec![3.0]];
    let y = [0, 0, 1, 1];
    let fitted = fit_logistic(
        &x,
        &y,
        &Penalty::Uniform(0.1),
        DEFAULT_MAX_ITER,
        DEFAULT_TOL,
    )?;
    assert!(fitted.converged);
    let midpoint = size_at_probability(&fitted, 0.5)?.ok_or("no midpoint")?;
    assert!(1.0 < midpoint && midpoint < 2.0);
    Ok(())
}

#[test]
fn test_two_feature_fit_and_predict() -> TestResult {
    let mut rng = Random::new(7);
    let mut x = Vec::new();
    let mut y = Vec::new();
    for _ in 0..3000 {
        let a = uniform(&mut rng, -2.0, 2.0);
        let b = uniform(&mut rng, -2.0, 2.0);
        let p = 1.0 / (1.0 + (-(0.5 + 1.5 * a - 2.0 * b)).exp());
        x.push(vec![a, b]);
        y.push(i64::from(draw(&mut rng) < p));
    }
    let fitted = fit(&x, &y, Penalty::Uniform(0.0))?;
    assert!(fitted.converged);
    assert_eq!(fitted.coef.len(), 3);
    assert!(approx(fitted.coef[0], 0.5, 0.15));
    assert!(approx(fitted.coef[1], 1.5, 0.15));
    assert!(approx(fitted.coef[2], -2.0, 0.15));
    assert_eq!(size_at_probability(&fitted, 0.5)?, None);
    let expected = 1.0 / (1.0 + (-0.5_f64).exp());
    assert!(approx(predict(&fitted, &[0.0, 0.0])?, expected, 0.05));
    Ok(())
}

#[test]
fn test_predict_is_stable_and_bounded() -> TestResult {
    let fitted = fit(&[vec![0.0], vec![1.0]], &[0, 1], Penalty::Uniform(1.0))?;
    assert!(predict(&fitted, &[-1000.0])? > 0.0);
    assert!(predict(&fitted, &[-1000.0])? < 1e-12);
    assert!(predict(&fitted, &[1000.0])? > 1.0 - 1e-12);
    assert!(predict(&fitted, &[1000.0])? <= 1.0);
    Ok(())
}

#[test]
fn test_predict_checks_feature_count() -> TestResult {
    let fitted = fit(&[vec![0.0], vec![1.0]], &[0, 1], Penalty::Uniform(1.0))?;
    assert!(predict(&fitted, &[1.0, 2.0]).is_err());
    Ok(())
}

#[test]
fn test_size_at_probability_is_none_when_slope_is_zero() -> TestResult {
    let flat = LogisticFit {
        coef: vec![0.3, 0.0],
        n: 4,
        converged: true,
        iterations: 1,
        log_likelihood: 0.0,
        penalty: vec![0.0, 0.0],
    };
    assert_eq!(size_at_probability(&flat, 0.5)?, None);
    Ok(())
}

#[test]
fn test_size_at_probability_rejects_bad_p() -> TestResult {
    let fitted = fit(&[vec![0.0], vec![1.0]], &[0, 1], Penalty::Uniform(1.0))?;
    for bad in [0.0, 1.0, -0.1, 1.5] {
        assert!(size_at_probability(&fitted, bad).is_err(), "p={bad}");
    }
    Ok(())
}

#[test]
fn test_fit_rejects_invalid_input() {
    let cases: [(Vec<Vec<f64>>, Vec<i64>); 4] = [
        (vec![], vec![]),
        (vec![vec![1.0]], vec![0, 1]),
        (vec![vec![1.0], vec![1.0, 2.0]], vec![0, 1]),
        (vec![vec![1.0], vec![2.0]], vec![0, 2]),
    ];
    for (x, y) in cases {
        assert!(fit_logistic(&x, &y, &Penalty::Uniform(0.0), 100, DEFAULT_TOL).is_err());
    }
}

// Bootstrap ------------------------------------------------------------------------------------

fn mean_statistic(sample: &[&f64]) -> Option<f64> {
    let total = wg_experiments::stats::py_sum(sample.iter().map(|v| **v));
    Some(total / sample.len() as f64)
}

#[test]
fn test_bootstrap_is_deterministic_for_a_seed() -> TestResult {
    let rows: Vec<f64> = (0..40).map(f64::from).collect();
    let config = |seed| BootstrapConfig {
        n: 200,
        seed,
        ..BootstrapConfig::default()
    };
    let first = bootstrap_interval(&rows, mean_statistic, &config(3))?;
    let second = bootstrap_interval(&rows, mean_statistic, &config(3))?;
    let other = bootstrap_interval(&rows, mean_statistic, &config(4))?;
    assert_eq!(first, second);
    let (low, high, produced) = first.ok_or("no interval")?;
    assert_eq!(produced, 200);
    assert!(low < 19.5 && 19.5 < high);
    assert_ne!(first, other);
    Ok(())
}

#[test]
fn test_bootstrap_resamples_whole_clusters() -> TestResult {
    let rows: Vec<(String, i64)> = (0..6)
        .flat_map(|i| (0..2).map(move |j| (format!("task-{i}"), j)))
        .collect();
    let seen: RefCell<Vec<(usize, bool)>> = RefCell::new(Vec::new());
    let statistic = |sample: &[&(String, i64)]| {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for row in sample {
            *counts.entry(row.0.as_str()).or_insert(0) += 1;
        }
        let all_whole = counts.values().all(|c| c % 2 == 0);
        seen.borrow_mut().push((sample.len(), all_whole));
        Some(counts.len() as f64)
    };
    let config = BootstrapConfig {
        n: 100,
        seed: 0,
        ..BootstrapConfig::default()
    };
    let result = bootstrap_clusters(&rows, |row| row.0.clone(), statistic, &config)?;
    assert!(result.is_some());
    // Six clusters of two rows are drawn per resample, so every sample has 12 rows, and each
    // task appears an even number of times because its two rows are always drawn together.
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 100);
    assert!(seen.iter().all(|&(length, whole)| length == 12 && whole));
    Ok(())
}

#[test]
fn test_bootstrap_skips_none_results() -> TestResult {
    let rows: Vec<f64> = (1..=10).map(f64::from).collect();
    let statistic = |sample: &[&f64]| {
        let total = wg_experiments::stats::py_sum(sample.iter().map(|v| **v));
        if total % 3.0 == 0.0 {
            None
        } else {
            Some(total)
        }
    };
    let config = BootstrapConfig {
        n: 300,
        seed: 0,
        ..BootstrapConfig::default()
    };
    let (low, high, produced) =
        bootstrap_interval(&rows, statistic, &config)?.ok_or("no interval")?;
    assert!(150 < produced && produced < 300);
    assert!(low <= high);
    Ok(())
}

#[test]
fn test_bootstrap_returns_none_when_most_resamples_fail() -> TestResult {
    let config = BootstrapConfig {
        n: 20,
        seed: 0,
        ..BootstrapConfig::default()
    };
    let rows = [1.0_f64, 2.0];
    assert_eq!(bootstrap_interval(&rows, |_| None, &config)?, None);
    Ok(())
}

#[test]
fn test_bootstrap_empty_rows_is_none() -> TestResult {
    let rows: Vec<f64> = Vec::new();
    assert_eq!(
        bootstrap_interval(&rows, mean_statistic, &BootstrapConfig::default())?,
        None
    );
    Ok(())
}

// Calibration ----------------------------------------------------------------------------------

#[test]
fn test_brier_on_small_input() -> TestResult {
    assert!(approx(
        brier(&[0.5, 1.0, 0.0], &[1, 1, 0])?,
        0.25 / 3.0,
        1e-15
    ));
    Ok(())
}

#[test]
fn test_brier_rejects_bad_input() {
    assert!(brier(&[], &[]).is_err());
    assert!(brier(&[0.5], &[1, 0]).is_err());
    assert!(brier(&[1.5], &[1]).is_err());
}

#[test]
fn test_reliability_bins_including_p_equal_one() -> TestResult {
    let bins = reliability(&[0.05, 0.15, 1.0, 0.95], &[0, 1, 1, 1], 10)?;
    assert_eq!(bins.len(), 10);
    assert_eq!(
        bins[0],
        ReliabilityBin {
            lo: 0.0,
            hi: 0.1,
            n: 1,
            mean_predicted: Some(0.05),
            observed: Some(0.0),
        }
    );
    assert_eq!(bins[1].n, 1);
    assert_eq!(bins[1].observed, Some(1.0));
    assert_eq!(bins[9].n, 2);
    assert!(approx(bins[9].mean_predicted.ok_or("empty")?, 0.975, 1e-12));
    assert_eq!(bins[9].observed, Some(1.0));
    assert_eq!(
        bins[5],
        ReliabilityBin {
            lo: 0.5,
            hi: 0.6,
            n: 0,
            mean_predicted: None,
            observed: None,
        }
    );
    assert_eq!(bins.iter().map(|b| b.n).sum::<usize>(), 4);
    Ok(())
}

#[test]
fn test_reliability_rejects_zero_bins() {
    assert!(reliability(&[0.5], &[1], 0).is_err());
}
