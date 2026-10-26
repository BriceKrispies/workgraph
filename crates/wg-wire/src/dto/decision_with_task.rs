//! `DecisionWithTaskOut`: `DecisionOut` plus the task it belongs to.

use wg_json::Value;
use wg_model::entity::task::TaskDecision;

use crate::convention::model;
use crate::dto::decision;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionWithTaskOut";

/// The fields in Python declaration order: the `DecisionOut` fields, then `task`.
pub const FIELDS: [&str; 16] = [
    "id",
    "task_id",
    "task_key",
    "question",
    "options",
    "status",
    "chosen_option_id",
    "other_text",
    "note",
    "decided_by",
    "decided_at",
    "created_by",
    "created_at",
    "updated_at",
    "version",
    "task",
];

/// A decision with its task. `task` is the `DecisionTaskOut` value built by the caller.
pub fn from_model(decision: &TaskDecision, task_key: &str, task: Value) -> Value {
    let mut fields = decision::fields(decision, task_key);
    fields.push(("task", task));
    model(NAME, fields)
}
