//! `DecisionTaskOut`: the task a decision belongs to, as `DecisionWithTaskOut` shows it.

use wg_json::Value;
use wg_model::entity::task::{Task, TaskTag};

use wg_wire::convention::{model, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionTaskOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 6] = ["key", "title", "status", "priority", "tags", "description"];

/// The brief of a decision's task as the decision query reads it: key, title, status, priority,
/// description and tags (in the order given).
pub fn from_brief(
    key: &str,
    title: &str,
    status: &str,
    priority: &str,
    description: &str,
    tags: &[String],
) -> Value {
    model(
        NAME,
        vec![
            ("key", text(key)),
            ("title", text(title)),
            ("status", text(status)),
            ("priority", text(priority)),
            (
                "tags",
                Value::List(tags.iter().map(|tag| text(tag)).collect()),
            ),
            ("description", text(description)),
        ],
    )
}

/// The task's key, title, status, priority, tags (in the order given) and description.
pub fn from_model(task: &Task, tags: &[TaskTag]) -> Value {
    model(
        NAME,
        vec![
            ("key", text(&task.key.to_string())),
            ("title", text(&task.title)),
            ("status", text(task.status.as_str())),
            ("priority", text(task.priority.as_str())),
            (
                "tags",
                Value::List(tags.iter().map(|tag| text(&tag.tag)).collect()),
            ),
            ("description", text(&task.description)),
        ],
    )
}
