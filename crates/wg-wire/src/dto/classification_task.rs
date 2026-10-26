//! `TaskClassifications`: the current classification and the history of a task, as
//! `classification_get` returns it.

use wg_json::Value;

use crate::convention::{model, text};

/// The name of the Pydantic model this DTO mirrors.
pub const NAME: &str = "TaskClassifications";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 6] = [
    "task_id",
    "task_key",
    "current",
    "history",
    "disagreement",
    "note",
];

/// The note the model carries on every answer: model results are inferences.
pub const NOTE: &str = "Model classifications are inferences, not objective facts.";

/// The classifications of a task. `current` is `None` when the task has none; `history` holds the
/// [`super::classification_out`] values, newest first.
pub fn from_parts(
    task_id: &str,
    task_key: &str,
    current: Option<Value>,
    history: Vec<Value>,
    disagreement: bool,
) -> Value {
    model(
        NAME,
        vec![
            ("task_id", text(task_id)),
            ("task_key", text(task_key)),
            ("current", current.unwrap_or(Value::Null)),
            ("history", Value::List(history)),
            ("disagreement", Value::Bool(disagreement)),
            ("note", text(NOTE)),
        ],
    )
}
