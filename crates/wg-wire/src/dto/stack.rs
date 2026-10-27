//! `StackOut`.

use wg_json::Value;
use wg_model::entity::stack::{Stack, StackItem, StackRepository};
use wg_model::entity::task::Task;
use wg_types::{Id, TaskKey, Timestamp};

use crate::convention::{display, model_z};
use crate::dto::Dto;
use crate::dto::stack_item::StackItemOut;
use crate::dto::stack_repository::StackRepositoryOut;
use crate::wire_dto;

wire_dto! {
    /// A stack as the API returns it.
    pub struct StackOut<'a>("StackOut") {
        pub id: Id,
        pub workspace_id: Id,
        pub parent_task_id: Id,
        #[serde(serialize_with = "display")]
        pub parent_key: &'a TaskKey,
        pub parent_title: &'a str,
        pub status: &'a str,
        pub repositories: Vec<StackRepositoryOut<'a>>,
        pub items: Vec<StackItemOut<'a>>,
        #[serde(serialize_with = "model_z")]
        pub created_at: Timestamp,
        #[serde(serialize_with = "model_z")]
        pub updated_at: Timestamp,
    }
}

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
    StackOut {
        id: stack.id,
        workspace_id: stack.workspace_id,
        parent_task_id: stack.parent_task_id,
        parent_key: &parent.key,
        parent_title: &parent.title,
        status: stack.status.as_str(),
        repositories: repositories
            .into_iter()
            .map(|(row, name, tip)| StackRepositoryOut::new(row, name, tip))
            .collect(),
        items: items
            .into_iter()
            .map(|(item, task)| StackItemOut::new(item, task))
            .collect(),
        created_at: stack.created_at,
        updated_at: stack.updated_at,
    }
    .value()
}
