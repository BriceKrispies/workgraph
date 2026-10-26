//! `StackOut`.

use wg_json::Value;
use wg_model::entity::stack::{Stack, StackItem, StackRepository};
use wg_model::entity::task::Task;

use crate::convention::{TimeFormat, id, model, text, timestamp};
use crate::dto::{stack_item, stack_repository};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "StackOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "workspace_id",
    "parent_task_id",
    "parent_key",
    "parent_title",
    "status",
    "repositories",
    "items",
    "created_at",
    "updated_at",
];

/// A stack as the API returns it. `parent` is the stack's parent task. `repositories` pairs each
/// repository row with its repository name, and `items` pairs each item with its child task; the
/// caller orders them (repositories by name, items by merge order with unordered items last, then
/// task number), as the Python `stack_out` does.
pub fn from_model<'a>(
    stack: &Stack,
    parent: &Task,
    repositories: impl IntoIterator<Item = (&'a StackRepository, &'a str)>,
    items: impl IntoIterator<Item = (&'a StackItem, &'a Task)>,
) -> Value {
    with_tips(
        stack,
        parent,
        repositories
            .into_iter()
            .map(|(row, name)| (row, name, None)),
        items,
    )
}

/// A stack with the integration tip of each repository (`StackService._with_tips`): the third
/// member of a repository is the tip its prepared worktree reports, `None` without one.
pub fn with_tips<'a>(
    stack: &Stack,
    parent: &Task,
    repositories: impl IntoIterator<Item = (&'a StackRepository, &'a str, Option<&'a str>)>,
    items: impl IntoIterator<Item = (&'a StackItem, &'a Task)>,
) -> Value {
    model(
        NAME,
        vec![
            ("id", id(stack.id)),
            ("workspace_id", id(stack.workspace_id)),
            ("parent_task_id", id(stack.parent_task_id)),
            ("parent_key", text(&parent.key.to_string())),
            ("parent_title", text(&parent.title)),
            ("status", text(stack.status.as_str())),
            (
                "repositories",
                Value::List(
                    repositories
                        .into_iter()
                        .map(|(row, name, tip)| stack_repository::with_tip(row, name, tip))
                        .collect(),
                ),
            ),
            (
                "items",
                Value::List(
                    items
                        .into_iter()
                        .map(|(item, task)| stack_item::from_model(item, task))
                        .collect(),
                ),
            ),
            (
                "created_at",
                timestamp(stack.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(stack.updated_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
