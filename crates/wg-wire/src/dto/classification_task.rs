//! `TaskClassifications`: the current classification and the history of a task, as
//! `classification_get` returns it.

use wg_json::Value;

use crate::wire_dto;

/// The note the model carries on every answer: model results are inferences.
pub const NOTE: &str = "Model classifications are inferences, not objective facts.";

wire_dto! {
    /// The classifications of a task. `current` is `None` when the task has none; `history` holds
    /// the [`super::classification_out`] values, newest first.
    pub struct TaskClassifications<'a>("TaskClassifications")
    value from_parts(task_id: &'a str, task_key: &'a str, current: Option<Value>, history: Vec<Value>, disagreement: bool) {
        task_id: &'a str = task_id,
        task_key: &'a str = task_key,
        current: Option<Value> = current,
        history: Vec<Value> = history,
        disagreement: bool = disagreement,
        note: &'static str = NOTE,
    }
}
