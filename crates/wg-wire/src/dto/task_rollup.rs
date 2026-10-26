//! `TaskRollup`.

use wg_json::{PyStr, Value};

use crate::convention::model;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskRollup";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["total_descendants", "by_status", "accepted", "progress"];

/// The progress of a task's descendants. `by_status` holds the counts in the order given.
pub fn from_model(
    total_descendants: i64,
    by_status: &[(String, i64)],
    accepted: i64,
    progress: f64,
) -> Value {
    model(
        NAME,
        vec![
            (
                "total_descendants",
                Value::Int(i128::from(total_descendants)),
            ),
            (
                "by_status",
                Value::Dict(
                    by_status
                        .iter()
                        .map(|(status, count)| {
                            (PyStr::from(status.as_str()), Value::Int(i128::from(*count)))
                        })
                        .collect(),
                ),
            ),
            ("accepted", Value::Int(i128::from(accepted))),
            ("progress", Value::Float(progress)),
        ],
    )
}
