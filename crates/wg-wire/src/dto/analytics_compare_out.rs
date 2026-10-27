//! The result of `POST /analytics/compare`: one metric over two periods, with the change per group.

use wg_json::{PyStr, Value};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, timestamp};
use crate::wire_dto;

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

wire_dto! {
    /// A comparison. The periods print as `isoformat()` of the bounds (aware UTC from the request).
    pub struct CompareOut<'a>("AnalyticsCompareOut")
    dict from_parts(
        metric: Value,
        statistic: &'a str,
        period_a: (Timestamp, Timestamp),
        period_b: (Timestamp, Timestamp),
        rows: Vec<Value>,
        notes: &'a [&'a str]
    ) {
        metric: Value = metric,
        statistic: &'a str = statistic,
        #[serde(serialize_with = "bounds")]
        period_a: (Timestamp, Timestamp) = period_a,
        #[serde(serialize_with = "bounds")]
        period_b: (Timestamp, Timestamp) = period_b,
        rows: Vec<Value> = rows,
        notes: &'a [&'a str] = notes,
    }
}

/// A `[since, until]` pair of `isoformat()` bounds.
fn bounds<S: serde::Serializer>(
    period: &(Timestamp, Timestamp),
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let at = |ts| timestamp(ts, TimeFormat::Isoformat);
    serde::Serialize::serialize(&Value::List(vec![at(period.0), at(period.1)]), serializer)
}
