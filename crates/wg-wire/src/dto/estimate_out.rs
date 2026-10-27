//! The result of `POST /analytics/estimate` (`estimate`): distributions of comparable accepted
//! tasks, or a refusal when too few are comparable.

use wg_json::{Value, dict};

use crate::convention::{text, texts};
use crate::wire_dto;

/// The name of the estimate dict; it has no OpenAPI component schema.
pub const NAME: &str = "EstimateOut";

/// The fields of the ok variant, in order.
pub const OK_FIELDS: [&str; 9] = [
    "status",
    "basis",
    "sample_size",
    "method",
    "distributions",
    "verification",
    "agent_recommendations",
    "uncertainty",
    "caveats",
];

/// The keys of `distributions`, in order.
pub const DISTRIBUTIONS_FIELDS: [&str; 7] = [
    "execution_ms",
    "queue_ms",
    "elapsed_ms",
    "attempts",
    "files_changed",
    "cumulative_churn",
    "final_change_size",
];

/// The fields of one distribution (`distribution`), in order.
pub const DISTRIBUTION_FIELDS: [&str; 7] = ["n", "p25", "median", "p75", "p90", "min", "max"];

wire_dto! {
    /// The refusal: fewer comparable tasks than the minimum sample.
    pub struct Insufficient<'a>("EstimateInsufficient")
    dict insufficient(message: &'a str, comparable_tasks: Vec<Value>, min_sample: i64) {
        status: &'static str = "insufficient_data",
        message: &'a str = message,
        comparable_tasks: Vec<Value> = comparable_tasks,
        min_sample: i64 = min_sample,
    }
}

wire_dto! {
    /// One comparable-task entry of `basis`.
    pub struct BasisRow<'a>("EstimateBasis")
    dict basis_row(task_id: &'a str, key: &'a str, title: &'a str, score: f64, reasons: &'a [&'a str]) {
        task_id: &'a str = task_id,
        key: &'a str = key,
        title: &'a str = title,
        score: f64 = score,
        reasons: &'a [&'a str] = reasons,
    }
}

wire_dto! {
    /// The `verification` block: the number of runs, how many passed and the rate, if any run.
    pub struct VerificationBlock("EstimateVerification")
    dict verification(runs: i64, passed: i64, pass_rate: Option<f64>) {
        runs: i64 = runs,
        passed: i64 = passed,
        pass_rate: Option<f64> = pass_rate,
    }
}

/// The ok estimate. `status` is always `ok`; `values` holds the other eight fields in
/// [`OK_FIELDS`] order (`basis`, `sample_size`, `method`, `distributions`, `verification`,
/// `agent_recommendations`, `uncertainty`, `caveats`), each already built by the caller.
pub fn ok(values: [Value; 8]) -> Value {
    let mut fields = vec![("status", text("ok"))];
    fields.extend(OK_FIELDS[1..].iter().copied().zip(values));
    dict(fields)
}

/// The `caveats` list of an estimate.
pub fn caveats(caveats: &[&str]) -> Value {
    texts(caveats.iter().copied())
}

/// One `distribution` block; the values are in [`DISTRIBUTION_FIELDS`] order.
pub fn distribution(values: [Value; 7]) -> Value {
    dict(DISTRIBUTION_FIELDS.into_iter().zip(values).collect())
}

/// The `distributions` block; the values are in [`DISTRIBUTIONS_FIELDS`] order.
pub fn distributions(values: [Value; 7]) -> Value {
    dict(DISTRIBUTIONS_FIELDS.into_iter().zip(values).collect())
}
