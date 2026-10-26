//! The result of `POST /analytics/estimate` (`estimate`): distributions of comparable accepted
//! tasks, or a refusal when too few are comparable.

use wg_json::Value;

use crate::convention::{dict, int, optional_float, text, texts};

/// The name of the estimate dict; it has no OpenAPI component schema.
pub const NAME: &str = "EstimateOut";

/// The fields of the insufficient-data variant, in order.
pub const INSUFFICIENT_FIELDS: [&str; 4] = ["status", "message", "comparable_tasks", "min_sample"];

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

/// The fields of one comparable-task entry of `basis`, in order.
pub const BASIS_FIELDS: [&str; 5] = ["task_id", "key", "title", "score", "reasons"];

/// The fields of `verification`, in order.
pub const VERIFICATION_FIELDS: [&str; 3] = ["runs", "passed", "pass_rate"];

/// The refusal: fewer comparable tasks than the minimum sample.
pub fn insufficient(message: &str, comparable_tasks: Vec<Value>, min_sample: i64) -> Value {
    dict(vec![
        ("status", text("insufficient_data")),
        ("message", text(message)),
        ("comparable_tasks", Value::List(comparable_tasks)),
        ("min_sample", int(min_sample)),
    ])
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

/// One comparable-task entry of `basis`.
pub fn basis_row(task_id: &str, key: &str, title: &str, score: f64, reasons: &[&str]) -> Value {
    dict(vec![
        ("task_id", text(task_id)),
        ("key", text(key)),
        ("title", text(title)),
        ("score", Value::Float(score)),
        ("reasons", texts(reasons.iter().copied())),
    ])
}

/// The `verification` block: the number of runs, how many passed and the rate, if any run.
pub fn verification(runs: i64, passed: i64, pass_rate: Option<f64>) -> Value {
    dict(vec![
        ("runs", int(runs)),
        ("passed", int(passed)),
        ("pass_rate", optional_float(pass_rate)),
    ])
}
