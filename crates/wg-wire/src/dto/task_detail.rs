//! `TaskDetail`: `TaskOut` plus the relations, comments, rollup and dependency state.

use wg_json::Value;
use wg_model::entity::task::{Task, TaskComment, TaskRelation};

use crate::convention::model;
use crate::dto::task::{self, TaskRelated};
use crate::dto::{comment, relation};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskDetail";

/// The fields in Python declaration order: the `TaskOut` fields, then the four of `TaskDetail`.
pub const FIELDS: [&str; 41] = [
    "id",
    "workspace_id",
    "key",
    "number",
    "title",
    "description",
    "acceptance_criteria",
    "status",
    "priority",
    "category",
    "tags",
    "files",
    "project_ids",
    "repository_ids",
    "parent_id",
    "child_ids",
    "depends_on",
    "blocks",
    "assignee",
    "assigned_agent",
    "claimed_by",
    "claim_expires_at",
    "auto_execute",
    "estimated_effort",
    "links",
    "metadata",
    "due_at",
    "scheduled_at",
    "created_by",
    "version",
    "created_at",
    "updated_at",
    "started_at",
    "completed_at",
    "accepted_at",
    "archived_at",
    "current_classification_id",
    "relations",
    "comments",
    "rollup",
    "dependency_state",
];

/// The task with its relations and comments. `rollup` and `dependency_state` are the values the
/// caller computed (see `TaskRollup` and `DependencyState`), or `None` when the task has none.
pub fn from_model(
    task: &Task,
    related: &TaskRelated<'_>,
    relations: &[TaskRelation],
    comments: &[TaskComment],
    rollup: Option<Value>,
    dependency_state: Option<Value>,
) -> Value {
    let mut fields = task::fields(task, related);
    fields.push((
        "relations",
        Value::List(relations.iter().map(relation::from_model).collect()),
    ));
    fields.push((
        "comments",
        Value::List(comments.iter().map(comment::from_model).collect()),
    ));
    fields.push(("rollup", rollup.unwrap_or(Value::Null)));
    fields.push(("dependency_state", dependency_state.unwrap_or(Value::Null)));
    model(NAME, fields)
}
