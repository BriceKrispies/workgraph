//! `TaskBrief`.

use wg_json::Value;
use wg_model::entity::task::Task;

use crate::convention::{id, model, optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskBrief";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 6] = ["id", "key", "title", "status", "priority", "category"];

/// The short form of a task, as a dependency or a graph neighbour is shown.
pub fn from_model(task: &Task) -> Value {
    model(
        NAME,
        vec![
            ("id", id(task.id)),
            ("key", text(&task.key.to_string())),
            ("title", text(&task.title)),
            ("status", text(task.status.as_str())),
            ("priority", text(task.priority.as_str())),
            ("category", optional_text(task.category.as_deref())),
        ],
    )
}
