//! The MCP output shapes of the experiment tools (CF-03): the dicts `experiment_list`,
//! `experiment_get`, `experiment_next`, `trial_start` and `experiment_report` return.
//!
//! These are dicts in Python, not DTOs, so they are not listed in [`super::implemented`]. The
//! statistics of a report (band rows, the size fit, usage) are computed by the core crates and
//! arrive here already as values; this module fixes only their keys and order.

use wg_json::{Value, dict};
use wg_model::entity::JsonObject;

use crate::convention::{counts, int, json_object, optional_float, optional_text, text};

/// `experiment_list`: `{"experiments": [ExperimentOut, ...]}`.
pub fn experiments(items: Vec<Value>) -> Value {
    dict!({ "experiments": Value::List(items) })
}

/// `experiment_get`: `{"experiment": ExperimentOut, "trials": [TrialOut, ...]}`.
pub fn experiment(experiment: Value, trials: Vec<Value>) -> Value {
    dict!({
        "experiment": experiment,
        "trials": Value::List(trials),
    })
}

/// The staircase state of a `Step` dict: `phase`, `band`, `reversals`, `threshold`, `visits`.
/// Each visit is a `(band, passes, trials)` triple, printed as a list.
pub fn step(
    phase: &str,
    band: Option<i64>,
    reversals: i64,
    threshold: Option<f64>,
    visits: &[(i64, i64, i64)],
) -> Value {
    dict!({
        "phase": text(phase),
        "band": band.map_or(Value::Null, int),
        "reversals": int(reversals),
        "threshold": optional_float(threshold),
        "visits": Value::List(
            visits
                .iter()
                .map(|(band, passes, trials)| {
                    Value::List(vec![int(*band), int(*passes), int(*trials)])
                })
                .collect(),
        ),
    })
}

/// `experiment_next`: `{"done", "phase", "trial", "reason", "step"}`. `trial` is `TrialOut` or
/// `null`; `phase` is `null` when the experiment is done.
pub fn next_plan(
    done: bool,
    phase: Option<&str>,
    trial: Option<Value>,
    reason: &str,
    step: Value,
) -> Value {
    dict!({
        "done": Value::Bool(done),
        "phase": optional_text(phase),
        "trial": trial.unwrap_or(Value::Null),
        "reason": text(reason),
        "step": step,
    })
}

/// `trial_start`: `{"trial", "worktree", "branch", "base_sha", "brief"}`.
pub fn trial_start(
    trial: Value,
    worktree: &str,
    branch: &str,
    base_sha: &str,
    brief: &str,
) -> Value {
    dict!({
        "trial": trial,
        "worktree": text(worktree),
        "branch": text(branch),
        "base_sha": text(base_sha),
        "brief": text(brief),
    })
}

/// One `band_rows` entry of an arm report: `band`, `n`, `passes`, `rate`, `wilson` (a
/// `[low, high]` pair).
pub fn band_row(band: i64, n: i64, passes: i64, rate: f64, wilson: (f64, f64)) -> Value {
    dict!({
        "band": int(band),
        "n": int(n),
        "passes": int(passes),
        "rate": Value::Float(rate),
        "wilson": Value::List(vec![Value::Float(wilson.0), Value::Float(wilson.1)]),
    })
}

/// The `trials` entry of an arm report: `total`, `by_status`, `by_phase`.
pub fn trial_counts(total: i64, by_status: &[(String, i64)], by_phase: &[(String, i64)]) -> Value {
    dict!({
        "total": int(total),
        "by_status": counts(by_status),
        "by_phase": counts(by_phase),
    })
}

/// One arm of `experiment_report`, with its computed parts already built.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmReport {
    pub arm: String,
    /// The [`trial_counts`] dict.
    pub trials: Value,
    /// A list of [`band_row`] dicts.
    pub bands: Value,
    /// The size fit dict, or `null`.
    pub fit: Value,
    pub fit_reason: Option<String>,
    pub usage: Value,
    /// Failure cause to count, sorted by cause.
    pub failure_causes: Vec<(String, i64)>,
    /// A list of invalid-trial dicts (`seq`, `task_key`, `failure_cause`, `notes`).
    pub invalid: Value,
}

/// An [`ArmReport`] as its dict: `arm`, `trials`, `bands`, `fit`, `fit_reason`, `usage`,
/// `failure_causes`, `invalid`.
pub fn arm(report: &ArmReport) -> Value {
    dict!({
        "arm": text(&report.arm),
        "trials": report.trials.clone(),
        "bands": report.bands.clone(),
        "fit": report.fit.clone(),
        "fit_reason": optional_text(report.fit_reason.as_deref()),
        "usage": report.usage.clone(),
        "failure_causes": counts(&report.failure_causes),
        "invalid": report.invalid.clone(),
    })
}

/// `experiment_report`: `{"experiment": {"slug", "status", "protocol"}, "step", "arms"}`.
pub fn report(
    slug: &str,
    status: &str,
    protocol: &JsonObject,
    step: Value,
    arms: Vec<Value>,
) -> Value {
    dict!({
        "experiment": dict!({
            "slug": text(slug),
            "status": text(status),
            "protocol": json_object(protocol),
        }),
        "step": step,
        "arms": Value::List(arms),
    })
}
