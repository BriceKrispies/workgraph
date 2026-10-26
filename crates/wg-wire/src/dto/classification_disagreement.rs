//! One disagreement of `GET /classification/disagreements`: a task whose succeeded
//! classifications name more than one category.

use wg_json::Value;
use wg_model::entity::classification::Classification;

use crate::convention::{dict, id, optional_float, optional_text, text};

/// The name of the disagreement dict; it has no OpenAPI component schema.
pub const NAME: &str = "ClassificationDisagreementOut";

/// The fields of one disagreement, in order.
pub const FIELDS: [&str; 3] = ["task_id", "task_key", "classifications"];

/// The fields of one classification inside a disagreement, in order.
pub const ITEM_FIELDS: [&str; 7] = [
    "id",
    "category",
    "source",
    "provider",
    "model",
    "is_current",
    "confidence",
];

/// One classification inside a disagreement.
pub fn item_from_model(row: &Classification) -> Value {
    dict(vec![
        ("id", id(row.id)),
        ("category", optional_text(row.category.as_deref())),
        ("source", text(&row.source)),
        ("provider", optional_text(row.provider.as_deref())),
        ("model", optional_text(row.model.as_deref())),
        ("is_current", Value::Bool(row.is_current)),
        ("confidence", optional_float(row.confidence.map(|c| c.0))),
    ])
}

/// One disagreement: the task and its classifications built by [`item_from_model`].
pub fn from_parts(task_id: &str, task_key: &str, classifications: Vec<Value>) -> Value {
    dict(vec![
        ("task_id", text(task_id)),
        ("task_key", text(task_key)),
        ("classifications", Value::List(classifications)),
    ])
}
