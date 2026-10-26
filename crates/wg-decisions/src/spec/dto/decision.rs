//! `DecisionOut`.

use wg_json::Value;
use wg_model::entity::task::TaskDecision;

use super::decision_option;
use wg_wire::convention::{
    TimeFormat, id, model, optional_text, optional_timestamp, text, timestamp,
};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 15] = [
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
];

/// The `DecisionOut` fields, in order, as name and value pairs. `DecisionWithTaskOut` extends
/// this list. `task_key` is the key of the decision's task, which the row does not hold.
pub fn fields(decision: &TaskDecision, task_key: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("id", id(decision.id)),
        ("task_id", id(decision.task_id)),
        ("task_key", text(task_key)),
        ("question", text(&decision.question)),
        (
            "options",
            Value::List(
                decision
                    .options
                    .iter()
                    .map(decision_option::from_model)
                    .collect(),
            ),
        ),
        ("status", text(&decision.status)),
        (
            "chosen_option_id",
            optional_text(decision.chosen_option_id.as_deref()),
        ),
        ("other_text", optional_text(decision.other_text.as_deref())),
        ("note", optional_text(decision.note.as_deref())),
        ("decided_by", optional_text(decision.decided_by.as_deref())),
        (
            "decided_at",
            optional_timestamp(decision.decided_at, TimeFormat::ModelZ),
        ),
        ("created_by", text(&decision.created_by)),
        (
            "created_at",
            timestamp(decision.created_at, TimeFormat::ModelZ),
        ),
        (
            "updated_at",
            timestamp(decision.updated_at, TimeFormat::ModelZ),
        ),
        ("version", Value::Int(i128::from(decision.version))),
    ]
}

/// A decision on a task, as the API returns it.
pub fn from_model(decision: &TaskDecision, task_key: &str) -> Value {
    model(NAME, fields(decision, task_key))
}
