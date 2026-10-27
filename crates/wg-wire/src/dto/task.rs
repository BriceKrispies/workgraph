//! `TaskOut`.

use rpds::VectorSync;
use serde_json::{Map, Value as Json};
use wg_model::entity::task::{Task, TaskFile, TaskLink, TaskTag};
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp};

use crate::convention::{display, json_map, json_map_opt, model_z};
use crate::dto::task_file::TaskFileOut;
use crate::dto::task_link::TaskLinkOut;
use crate::wire_dto;

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

wire_dto! {
    /// The task as the API returns it. Tags, files, links and the id lists come from the task's
    /// satellite tables and are given in the order the caller lists them.
    pub struct TaskOut<'a>("TaskOut") value from_model(t: &'a Task, r: &TaskRelated<'a>) {
        id: Id = t.id,
        workspace_id: Id = t.workspace_id,
        #[serde(serialize_with = "display")]
        key: &'a TaskKey = &t.key,
        number: i64 = t.number,
        title: &'a str = &t.title,
        description: &'a str = &t.description,
        acceptance_criteria: &'a VectorSync<String> = &t.acceptance_criteria,
        status: TaskStatus = t.status,
        priority: TaskPriority = t.priority,
        category: Option<&'a str> = t.category.as_deref(),
        tags: Vec<&'a str> = r.tags.iter().map(|tag| tag.tag.as_str()).collect(),
        files: Vec<TaskFileOut<'a>> = r.files.iter().map(TaskFileOut::new).collect(),
        project_ids: &'a [Id] = r.project_ids,
        repository_ids: &'a [Id] = r.repository_ids,
        parent_id: Option<Id> = t.parent_id,
        child_ids: &'a [Id] = r.child_ids,
        depends_on: &'a [Id] = r.depends_on,
        blocks: &'a [Id] = r.blocks,
        assignee: Option<&'a str> = t.assignee.as_deref(),
        assigned_agent: Option<&'a str> = t.assigned_agent.as_deref(),
        claimed_by: Option<&'a str> = t.claimed_by.as_deref(),
        #[serde(serialize_with = "model_z")]
        claim_expires_at: Option<Timestamp> = t.claim_expires_at,
        auto_execute: bool = t.auto_execute,
        #[serde(serialize_with = "json_map_opt")]
        estimated_effort: Option<&'a Map<String, Json>> = t.estimated_effort.as_ref(),
        links: Vec<TaskLinkOut<'a>> = r.links.iter().map(TaskLinkOut::new).collect(),
        #[serde(serialize_with = "json_map")]
        metadata: &'a Map<String, Json> = &t.metadata,
        #[serde(serialize_with = "model_z")]
        due_at: Option<Timestamp> = t.due_at,
        #[serde(serialize_with = "model_z")]
        scheduled_at: Option<Timestamp> = t.scheduled_at,
        created_by: &'a str = &t.created_by,
        version: i64 = t.version,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = t.created_at,
        #[serde(serialize_with = "model_z")]
        updated_at: Timestamp = t.updated_at,
        #[serde(serialize_with = "model_z")]
        started_at: Option<Timestamp> = t.started_at,
        #[serde(serialize_with = "model_z")]
        completed_at: Option<Timestamp> = t.completed_at,
        #[serde(serialize_with = "model_z")]
        accepted_at: Option<Timestamp> = t.accepted_at,
        #[serde(serialize_with = "model_z")]
        archived_at: Option<Timestamp> = t.archived_at,
        current_classification_id: Option<Id> = t.current_classification_id,
    }
}
