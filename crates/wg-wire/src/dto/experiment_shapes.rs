//! The MCP output shapes of the experiment tools (CF-03): the dicts `experiment_list`,
//! `experiment_get`, `experiment_next`, `trial_start` and `experiment_report` return.
//!
//! These are dicts in Python, not DTOs, so they are not listed in [`super::implemented`]. The
//! statistics of a report (band rows, the size fit, usage) are computed by the core crates and
//! arrive here already as values; this module fixes only their keys and order.

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::entity::JsonObject;

use crate::convention::{counts_field, json_map};
use crate::wire_dto;

wire_dto! {
    /// `experiment_list`: `{"experiments": [ExperimentOut, ...]}`.
    pub struct Experiments("Experiments") dict experiments(experiments: Vec<Value>) {
        experiments: Vec<Value> = experiments,
    }
}

wire_dto! {
    /// `experiment_get`: `{"experiment": ExperimentOut, "trials": [TrialOut, ...]}`.
    pub struct ExperimentGet("ExperimentGet") dict experiment(experiment: Value, trials: Vec<Value>) {
        experiment: Value = experiment,
        trials: Vec<Value> = trials,
    }
}

wire_dto! {
    /// The staircase state of a `Step` dict. Each visit is a `(band, passes, trials)` triple,
    /// printed as a list.
    pub struct StepOut<'a>("StepOut")
    dict step(phase: &'a str, band: Option<i64>, reversals: i64, threshold: Option<f64>, visits: &'a [(i64, i64, i64)]) {
        phase: &'a str = phase,
        band: Option<i64> = band,
        reversals: i64 = reversals,
        threshold: Option<f64> = threshold,
        visits: &'a [(i64, i64, i64)] = visits,
    }
}

wire_dto! {
    /// `experiment_next`. `trial` is `TrialOut` or `null`; `phase` is `null` when the experiment
    /// is done.
    pub struct NextPlan<'a>("NextPlan")
    dict next_plan(done: bool, phase: Option<&'a str>, trial: Option<Value>, reason: &'a str, step: Value) {
        done: bool = done,
        phase: Option<&'a str> = phase,
        trial: Option<Value> = trial,
        reason: &'a str = reason,
        step: Value = step,
    }
}

wire_dto! {
    /// `trial_start`.
    pub struct TrialStart<'a>("TrialStart")
    dict trial_start(trial: Value, worktree: &'a str, branch: &'a str, base_sha: &'a str, brief: &'a str) {
        trial: Value = trial,
        worktree: &'a str = worktree,
        branch: &'a str = branch,
        base_sha: &'a str = base_sha,
        brief: &'a str = brief,
    }
}

wire_dto! {
    /// One `band_rows` entry of an arm report; `wilson` is a `[low, high]` pair.
    pub struct BandRow("BandRow")
    dict band_row(band: i64, n: i64, passes: i64, rate: f64, wilson: (f64, f64)) {
        band: i64 = band,
        n: i64 = n,
        passes: i64 = passes,
        rate: f64 = rate,
        wilson: (f64, f64) = wilson,
    }
}

wire_dto! {
    /// The `trials` entry of an arm report.
    pub struct TrialCounts<'a>("TrialCounts")
    dict trial_counts(total: i64, by_status: &'a [(String, i64)], by_phase: &'a [(String, i64)]) {
        total: i64 = total,
        #[serde(serialize_with = "counts_field")]
        by_status: &'a [(String, i64)] = by_status,
        #[serde(serialize_with = "counts_field")]
        by_phase: &'a [(String, i64)] = by_phase,
    }
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

wire_dto! {
    /// An [`ArmReport`] as its dict: `arm`, `trials`, `bands`, `fit`, `fit_reason`, `usage`,
    /// `failure_causes`, `invalid`.
    pub struct ArmOut<'a>("ArmOut") dict arm(r: &'a ArmReport) {
        arm: &'a str = &r.arm,
        trials: &'a Value = &r.trials,
        bands: &'a Value = &r.bands,
        fit: &'a Value = &r.fit,
        fit_reason: Option<&'a str> = r.fit_reason.as_deref(),
        usage: &'a Value = &r.usage,
        #[serde(serialize_with = "counts_field")]
        failure_causes: &'a [(String, i64)] = &r.failure_causes,
        invalid: &'a Value = &r.invalid,
    }
}

wire_dto! {
    /// `experiment_report`: `{"experiment": {"slug", "status", "protocol"}, "step", "arms"}`.
    pub struct ReportOut<'a>("ReportOut")
    dict report(slug: &'a str, status: &'a str, protocol: &'a JsonObject, step: Value, arms: Vec<Value>) {
        experiment: ReportExperiment<'a> = ReportExperiment { slug, status, protocol },
        step: Value = step,
        arms: Vec<Value> = arms,
    }
}

wire_dto! {
    /// The `experiment` block of `experiment_report`.
    pub struct ReportExperiment<'a>("ReportExperiment") {
        pub slug: &'a str,
        pub status: &'a str,
        #[serde(serialize_with = "json_map")]
        pub protocol: &'a Map<String, Json>,
    }
}
