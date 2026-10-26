//! Updating a task. Mirrors `_update_in_session`, `update` and `bulk_update` in
//! `core/services/tasks.py`: the optimistic version check, the field-by-field diff with its
//! `changes` record, tag, file, project and parent edits, the content hash, the tracker guard,
//! and the status request the shell then runs through the lifecycle.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use wg_model::TaskSlice;
use wg_model::entity::{JsonObject, Task, TaskFile};
use wg_model::event::{Changes, TaskUpdated};
use wg_model::stamp::Stamp;
use wg_types::{ErrorCode, Id, TaskPriority, TaskStatus, Timestamp, WgError};

use crate::claims::check_version;
use crate::content::content_hash;
use crate::files::{FileInput, add_files, current_files, file_rows};
use crate::hierarchy::check_set_parent;
use crate::ownership::{guard_tracker_owned, tracked_by};

/// The fields of a `TaskUpdate`. `None` leaves a field alone; a nullable field carries
/// `Some(None)` to clear it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    /// The version the caller read; a different stored version fails the update.
    pub expected_version: Option<i64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub assignee: Option<Option<String>>,
    pub assigned_agent: Option<Option<String>>,
    pub auto_execute: Option<bool>,
    pub due_at: Option<Option<Timestamp>>,
    pub scheduled_at: Option<Option<Timestamp>>,
    pub priority: Option<TaskPriority>,
    pub estimated_effort: Option<JsonObject>,
    /// Merged over the stored metadata: keys given replace, other keys stay.
    pub metadata: Option<JsonObject>,
    /// Replaces the tag set before `add_tags` and `remove_tags` apply.
    pub tags: Option<Vec<String>>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    /// Replaces the file list before `add_files` applies.
    pub files: Option<Vec<FileInput>>,
    pub add_files: Vec<FileInput>,
    /// Replaces the project set, as resolved ids.
    pub projects: Option<Vec<Id>>,
    /// The parent, as a resolved id.
    pub parent: Option<Id>,
    pub clear_parent: bool,
    /// A status the task should move to. Applied by the shell through the lifecycle, after the
    /// update event.
    pub status: Option<TaskStatus>,
    pub reason: Option<String>,
}

/// A patch for a bulk update: the same change for every task, with no version check.
pub fn bulk_patch(patch: &TaskPatch) -> TaskPatch {
    TaskPatch {
        expected_version: None,
        ..patch.clone()
    }
}

/// The rows, event and status request of one update. Writes apply in this order: the task row,
/// then `tags`, `files` and `projects` when they are `Some`, then the event, then the status.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdatePlan {
    /// The patched task. `version` and `updated_at` move only when something changed.
    pub task: Task,
    pub changes: Changes,
    /// Whether the title or description changed after normalisation.
    pub material_change: bool,
    /// The new tag rows' tag names (deduplicated), when the tag set changed.
    pub tags: Option<Vec<String>>,
    /// The new `task_files` rows, when the file list changed.
    pub files: Option<Vec<TaskFile>>,
    /// The new project set, sorted, when it changed.
    pub projects: Option<Vec<Id>>,
    /// The status the task should move to, if the patch asked.
    pub status: Option<TaskStatus>,
    /// The `task.updated` event; `None` when nothing changed.
    pub event: Option<TaskUpdated>,
}

/// Plans an update of `task` (a row of `tasks`) with `patch`.
pub fn update_plan(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    patch: &TaskPatch,
) -> Result<UpdatePlan, WgError> {
    check_version(task, patch.expected_version)?;
    let mut next = task.clone();
    let mut log: Vec<(&'static str, Value, Value)> = Vec::new();

    if let Some(title) = &patch.title {
        replace(&mut log, "title", &mut next.title, title, text);
    }
    if let Some(description) = &patch.description {
        replace(
            &mut log,
            "description",
            &mut next.description,
            description,
            text,
        );
    }
    if let Some(criteria) = &patch.acceptance_criteria {
        let before: Vec<String> = next.acceptance_criteria.iter().cloned().collect();
        if before != *criteria {
            log.push(("acceptance_criteria", json!(before), json!(criteria)));
            next.acceptance_criteria = criteria.iter().cloned().collect();
        }
    }
    if let Some(assignee) = &patch.assignee {
        replace(&mut log, "assignee", &mut next.assignee, assignee, opt_text);
    }
    if let Some(agent) = &patch.assigned_agent {
        replace(
            &mut log,
            "assigned_agent",
            &mut next.assigned_agent,
            agent,
            opt_text,
        );
    }
    if let Some(auto_execute) = patch.auto_execute {
        replace(
            &mut log,
            "auto_execute",
            &mut next.auto_execute,
            &auto_execute,
            flag,
        );
    }
    if let Some(due_at) = &patch.due_at {
        replace(&mut log, "due_at", &mut next.due_at, due_at, timestamp_json);
    }
    if let Some(scheduled_at) = &patch.scheduled_at {
        replace(
            &mut log,
            "scheduled_at",
            &mut next.scheduled_at,
            scheduled_at,
            timestamp_json,
        );
    }
    if let Some(priority) = patch.priority {
        replace(
            &mut log,
            "priority",
            &mut next.priority,
            &priority,
            priority_json,
        );
    }
    if let Some(effort) = &patch.estimated_effort {
        replace(
            &mut log,
            "estimated_effort",
            &mut next.estimated_effort,
            &Some(effort.clone()),
            opt_object,
        );
    }
    if let Some(metadata) = &patch.metadata {
        let mut merged = next.metadata.clone();
        for (key, value) in metadata {
            merged.insert(key.clone(), value.clone());
        }
        replace(&mut log, "metadata", &mut next.metadata, &merged, object);
    }

    let current_tags: Vec<String> = tasks.tags_of(task.id).map(str::to_owned).collect();
    let mut new_tags = patch.tags.clone().unwrap_or_else(|| current_tags.clone());
    for tag in &patch.add_tags {
        if !new_tags.contains(tag) {
            new_tags.push(tag.clone());
        }
    }
    if !patch.remove_tags.is_empty() {
        new_tags.retain(|tag| !patch.remove_tags.contains(tag));
    }
    let tags = if new_tags != current_tags {
        log.push(("tags", json!(current_tags), json!(new_tags)));
        Some(dedup(&new_tags))
    } else {
        None
    };

    let current = current_files(tasks, task.id);
    let mut new_files = patch.files.clone().unwrap_or_else(|| current.clone());
    new_files = add_files(new_files, &patch.add_files);
    let files = if new_files != current {
        log.push((
            "files",
            Value::Array(current.iter().map(FileInput::to_json).collect()),
            Value::Array(new_files.iter().map(FileInput::to_json).collect()),
        ));
        Some(file_rows(task.id, &new_files))
    } else {
        None
    };

    let projects = match &patch.projects {
        Some(wanted) => {
            let mut before: Vec<Id> = tasks
                .task_projects
                .values()
                .filter(|link| link.task_id == task.id)
                .map(|link| link.project_id)
                .collect();
            before.sort();
            let mut after: Vec<Id> = dedup(wanted);
            after.sort();
            if before != after {
                log.push(("project_ids", ids_json(&before), ids_json(&after)));
                Some(after)
            } else {
                None
            }
        }
        None => None,
    };

    if patch.clear_parent && task.parent_id.is_some() {
        replace(
            &mut log,
            "parent_id",
            &mut next.parent_id,
            &None,
            parent_json,
        );
    } else if let Some(parent) = patch.parent {
        check_set_parent(tasks, task.id, parent)?;
        replace(
            &mut log,
            "parent_id",
            &mut next.parent_id,
            &Some(parent),
            parent_json,
        );
    }

    let new_hash = content_hash(&next.title, &next.description);
    let material_change = new_hash != next.description_hash;
    if material_change {
        next.description_hash = new_hash;
    }

    let mut touched: BTreeSet<&str> = log
        .iter()
        .map(|(field, _, _)| {
            if *field == "parent_id" {
                "parent"
            } else {
                *field
            }
        })
        .collect();
    if let Some(target) = patch.status {
        let closed = [TaskStatus::Accepted, TaskStatus::Cancelled];
        if target != task.status && (closed.contains(&target) || closed.contains(&task.status)) {
            touched.insert("status");
        }
    }
    guard_tracker_owned(&stamp.actor, task, &touched, tracked_by(tasks, task.id))?;

    if patch.status == Some(TaskStatus::Claimed) {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "use task_claim to claim a task",
        ));
    }

    let changes = log
        .into_iter()
        .fold(Changes::new(), |acc, (field, before, after)| {
            acc.with(field, before, after)
        });
    let event = if changes.is_empty() {
        None
    } else {
        next.version += 1;
        next.updated_at = stamp.at;
        Some(TaskUpdated {
            key: task.key.clone(),
            changes: changes.clone(),
            material_change,
            reason: patch.reason.clone(),
            version: next.version,
        })
    };

    Ok(UpdatePlan {
        task: next,
        changes,
        material_change,
        tags,
        files,
        projects,
        status: patch.status,
        event,
    })
}

/// Sets `slot` to `new` when they differ, recording `[before, after]` under `field`.
fn replace<T: Clone + PartialEq>(
    log: &mut Vec<(&'static str, Value, Value)>,
    field: &'static str,
    slot: &mut T,
    new: &T,
    encode: fn(&T) -> Value,
) {
    if *slot != *new {
        log.push((field, encode(slot), encode(new)));
        *slot = new.clone();
    }
}

fn text(value: &String) -> Value {
    json!(value)
}

fn opt_text(value: &Option<String>) -> Value {
    json!(value)
}

fn flag(value: &bool) -> Value {
    json!(value)
}

fn opt_object(value: &Option<JsonObject>) -> Value {
    json!(value)
}

fn object(value: &JsonObject) -> Value {
    Value::Object(value.clone())
}

fn timestamp_json(value: &Option<Timestamp>) -> Value {
    value.map_or(Value::Null, |at| json!(at.format_isoformat()))
}

fn priority_json(value: &TaskPriority) -> Value {
    json!(value.as_str())
}

fn parent_json(value: &Option<Id>) -> Value {
    value.map_or(Value::Null, |id| json!(id.to_string()))
}

fn ids_json(ids: &[Id]) -> Value {
    Value::Array(ids.iter().map(|id| json!(id.to_string())).collect())
}

/// The values in first-seen order without repeats (`dict.fromkeys`).
fn dedup<T: Clone + PartialEq>(values: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(values.len());
    for value in values {
        if !out.contains(value) {
            out.push(value.clone());
        }
    }
    out
}
