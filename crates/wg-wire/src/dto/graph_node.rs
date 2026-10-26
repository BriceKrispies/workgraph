//! `GraphNode`.

use wg_json::Value;
use wg_model::entity::task::{Task, TaskTag};
use wg_types::Id;

use crate::convention::{id, ids, model, optional_id, optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "GraphNode";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 11] = [
    "id",
    "key",
    "title",
    "status",
    "priority",
    "category",
    "parent_id",
    "project_ids",
    "repository_ids",
    "tags",
    "blocked",
];

/// One task of the dependency graph. `blocked` is the service's verdict (an unsatisfied
/// dependency); the satisfaction rules are not decided here.
pub fn from_model(
    task: &Task,
    project_ids: &[Id],
    repository_ids: &[Id],
    tags: &[TaskTag],
    blocked: bool,
) -> Value {
    model(
        NAME,
        vec![
            ("id", id(task.id)),
            ("key", text(&task.key.to_string())),
            ("title", text(&task.title)),
            ("status", text(task.status.as_str())),
            ("priority", text(task.priority.as_str())),
            ("category", optional_text(task.category.as_deref())),
            ("parent_id", optional_id(task.parent_id)),
            ("project_ids", ids(project_ids.iter().copied())),
            ("repository_ids", ids(repository_ids.iter().copied())),
            (
                "tags",
                Value::List(tags.iter().map(|tag| text(&tag.tag)).collect()),
            ),
            ("blocked", Value::Bool(blocked)),
        ],
    )
}
