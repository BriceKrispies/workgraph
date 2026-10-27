//! The subagent report of `GET /analytics/subagents` (MCP `analytics_subagents`): acceptance of
//! subagent runs by period, by model and by task size.

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, optional_timestamp, text};

/// The name of the report dict; it has no OpenAPI component schema.
pub const NAME: &str = "SubagentReportOut";

/// The fields of the report, in order.
pub const FIELDS: [&str; 8] = [
    "period",
    "window",
    "model",
    "by_period",
    "by_size",
    "totals",
    "unlinked_runs",
    "notes",
];

/// The measured fields shared by period rows, model totals and size rows (`_summary`), in order.
pub const SUMMARY_FIELDS: [&str; 13] = [
    "runs",
    "linked_runs",
    "tasks",
    "accepted",
    "rejected",
    "failed",
    "pending",
    "acceptance_rate",
    "wilson",
    "first_pass",
    "median_output_tokens",
    "median_turns",
    "tool_errors",
];

/// The fields of one size row, in order.
pub const BY_SIZE_FIELDS: [&str; 8] = [
    "model",
    "band",
    "tasks",
    "accepted",
    "decided",
    "acceptance_rate",
    "wilson",
    "median_lines",
];

/// The `window` block; the bounds are `isoformat()` of aware UTC datetimes.
pub fn window(since: Option<Timestamp>, until: Option<Timestamp>) -> Value {
    dict!({
        "since": optional_timestamp(since, TimeFormat::Isoformat),
        "until": optional_timestamp(until, TimeFormat::Isoformat),
    })
}

/// The summary pairs, `SUMMARY_FIELDS` order.
fn summary_pairs(values: [Value; 13]) -> Vec<(&'static str, Value)> {
    SUMMARY_FIELDS.into_iter().zip(values).collect()
}

/// A period row: `period`, `model`, then the summary.
pub fn period_row(period: &str, model: &str, summary: [Value; 13]) -> Value {
    let mut fields = vec![("period", text(period)), ("model", text(model))];
    fields.extend(summary_pairs(summary));
    dict(fields)
}

/// A model total: `model`, then the summary.
pub fn model_total(model: &str, summary: [Value; 13]) -> Value {
    let mut fields = vec![("model", text(model))];
    fields.extend(summary_pairs(summary));
    dict(fields)
}

/// A size row; the values are in [`BY_SIZE_FIELDS`] order after `model` is given first.
/// `band` is `None` for tasks without a measured size.
pub fn size_row(model: &str, band: Option<i64>, values: [Value; 6]) -> Value {
    let band_value = band.map_or(Value::Null, |band| Value::Int(i128::from(band)));
    let mut fields = vec![("model", text(model)), ("band", band_value)];
    fields.extend(BY_SIZE_FIELDS[2..].iter().copied().zip(values));
    dict(fields)
}

/// The report; the values are in [`FIELDS`] order.
pub fn from_parts(values: [Value; 8]) -> Value {
    dict(FIELDS.into_iter().zip(values).collect())
}
