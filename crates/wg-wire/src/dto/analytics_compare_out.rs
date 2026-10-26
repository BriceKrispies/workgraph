//! The result of `POST /analytics/compare`: one metric over two periods, with the change per group.

use wg_json::{PyStr, Value};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, dict, text, texts, timestamp};

/// The name of the result dict; it has no OpenAPI component schema.
pub const NAME: &str = "AnalyticsCompareOut";

/// The fields in the order `AnalyticsService.compare` builds them.
pub const FIELDS: [&str; 6] = [
    "metric",
    "statistic",
    "period_a",
    "period_b",
    "rows",
    "notes",
];

/// The fields of a comparison row after its group keys, in order.
pub const ROW_FIELDS: [&str; 6] = ["a", "b", "delta", "relative_change", "n_a", "n_b"];

/// One comparison row: the group keys (in `group_by` order), then the [`ROW_FIELDS`] values.
pub fn row(group: Vec<(String, Value)>, values: [Value; 6]) -> Value {
    let mut fields: Vec<(PyStr, Value)> = group
        .into_iter()
        .map(|(key, value)| (PyStr::from(key.as_str()), value))
        .collect();
    fields.extend(
        ROW_FIELDS
            .into_iter()
            .zip(values)
            .map(|(key, value)| (PyStr::from(key), value)),
    );
    Value::Dict(fields)
}

/// A comparison. The periods print as `isoformat()` of the bounds (aware UTC from the request).
pub fn from_parts(
    metric: Value,
    statistic: &str,
    period_a: (Timestamp, Timestamp),
    period_b: (Timestamp, Timestamp),
    rows: Vec<Value>,
    notes: &[&str],
) -> Value {
    let period = |(since, until): (Timestamp, Timestamp)| {
        Value::List(vec![
            timestamp(since, TimeFormat::Isoformat),
            timestamp(until, TimeFormat::Isoformat),
        ])
    };
    dict(vec![
        ("metric", metric),
        ("statistic", text(statistic)),
        ("period_a", period(period_a)),
        ("period_b", period(period_b)),
        ("rows", Value::List(rows)),
        ("notes", texts(notes.iter().copied())),
    ])
}
