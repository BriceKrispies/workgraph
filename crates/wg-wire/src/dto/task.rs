//! `TaskOut`.

use wg_json::Value;
use wg_model::entity::task::{Task, TaskFile, TaskLink, TaskTag};
use wg_types::Id;

use crate::convention::{
    TimeFormat, id, ids, json_object, model, optional_id, optional_json_object, optional_text,
    optional_timestamp, text, timestamp,
};
use crate::dto::{task_file, task_link};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 37] = [
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
];

/// The rows that belong to a task, or ids the service derives from other tables, passed to
/// [`from_model`] next to the task row itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct TaskRelated<'a> {
    pub tags: &'a [TaskTag],
    pub files: &'a [TaskFile],
    pub links: &'a [TaskLink],
    pub project_ids: &'a [Id],
    pub repository_ids: &'a [Id],
    pub child_ids: &'a [Id],
    pub depends_on: &'a [Id],
    pub blocks: &'a [Id],
}

/// The `TaskOut` fields, in order, as name and value pairs. `TaskDetail` extends this list.
pub fn fields(task: &Task, related: &TaskRelated<'_>) -> Vec<(&'static str, Value)> {
    let files: Vec<Value> = related.files.iter().map(task_file::from_model).collect();
    let links: Vec<Value> = related.links.iter().map(task_link::from_model).collect();
    vec![
        ("id", id(task.id)),
        ("workspace_id", id(task.workspace_id)),
        ("key", text(&task.key.to_string())),
        ("number", Value::Int(i128::from(task.number))),
        ("title", text(&task.title)),
        ("description", text(&task.description)),
        (
            "acceptance_criteria",
            Value::List(task.acceptance_criteria.iter().map(|c| text(c)).collect()),
        ),
        ("status", text(task.status.as_str())),
        ("priority", text(task.priority.as_str())),
        ("category", optional_text(task.category.as_deref())),
        (
            "tags",
            Value::List(related.tags.iter().map(|tag| text(&tag.tag)).collect()),
        ),
        ("files", Value::List(files)),
        ("project_ids", ids(related.project_ids.iter().copied())),
        (
            "repository_ids",
            ids(related.repository_ids.iter().copied()),
        ),
        ("parent_id", optional_id(task.parent_id)),
        ("child_ids", ids(related.child_ids.iter().copied())),
        ("depends_on", ids(related.depends_on.iter().copied())),
        ("blocks", ids(related.blocks.iter().copied())),
        ("assignee", optional_text(task.assignee.as_deref())),
        (
            "assigned_agent",
            optional_text(task.assigned_agent.as_deref()),
        ),
        ("claimed_by", optional_text(task.claimed_by.as_deref())),
        (
            "claim_expires_at",
            optional_timestamp(task.claim_expires_at, TimeFormat::ModelZ),
        ),
        ("auto_execute", Value::Bool(task.auto_execute)),
        (
            "estimated_effort",
            optional_json_object(task.estimated_effort.as_ref()),
        ),
        ("links", Value::List(links)),
        ("metadata", json_object(&task.metadata)),
        (
            "due_at",
            optional_timestamp(task.due_at, TimeFormat::ModelZ),
        ),
        (
            "scheduled_at",
            optional_timestamp(task.scheduled_at, TimeFormat::ModelZ),
        ),
        ("created_by", text(&task.created_by)),
        ("version", Value::Int(i128::from(task.version))),
        ("created_at", timestamp(task.created_at, TimeFormat::ModelZ)),
        ("updated_at", timestamp(task.updated_at, TimeFormat::ModelZ)),
        (
            "started_at",
            optional_timestamp(task.started_at, TimeFormat::ModelZ),
        ),
        (
            "completed_at",
            optional_timestamp(task.completed_at, TimeFormat::ModelZ),
        ),
        (
            "accepted_at",
            optional_timestamp(task.accepted_at, TimeFormat::ModelZ),
        ),
        (
            "archived_at",
            optional_timestamp(task.archived_at, TimeFormat::ModelZ),
        ),
        (
            "current_classification_id",
            optional_id(task.current_classification_id),
        ),
    ]
}

/// The task as the API returns it. Tags, files, links and the id lists come from the task's
/// satellite tables and are given in the order the caller lists them.
pub fn from_model(task: &Task, related: &TaskRelated<'_>) -> Value {
    model(NAME, fields(task, related))
}
