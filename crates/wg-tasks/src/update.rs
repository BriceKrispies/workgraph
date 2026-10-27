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
use wg_types::{Id, TaskPriority, TaskStatus, Timestamp, WgError};

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

    let title = pick(&patch.title, &task.title);
    let description = pick(&patch.description, &task.description);
    let assignee = pick(&patch.assignee, &task.assignee);
    let assigned_agent = pick(&patch.assigned_agent, &task.assigned_agent);
    let auto_execute = pick(&patch.auto_execute, &task.auto_execute);
    let due_at = pick(&patch.due_at, &task.due_at);
    let scheduled_at = pick(&patch.scheduled_at, &task.scheduled_at);
    let priority = pick(&patch.priority, &task.priority);
    let estimated_effort = patch
        .estimated_effort
        .clone()
        .map_or_else(|| task.estimated_effort.clone(), Some);
    // Keys given replace the stored ones; other keys stay.
    let metadata = patch.metadata.as_ref().map_or_else(
        || task.metadata.clone(),
        |given| {
            task.metadata
                .iter()
                .chain(given.iter())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        },
    );

    let criteria_before: Vec<String> = task.acceptance_criteria.iter().cloned().collect();
    let criteria_after: Vec<String> = patch
        .acceptance_criteria
        .clone()
        .unwrap_or_else(|| criteria_before.clone());

    let current_tags: Vec<String> = tasks.tags_of(task.id).map(str::to_owned).collect();
    let with_added = patch.add_tags.iter().fold(
        patch.tags.clone().unwrap_or_else(|| current_tags.clone()),
        |tags, tag| {
            if tags.contains(tag) {
                tags
            } else {
                tags.into_iter().chain([tag.clone()]).collect()
            }
        },
    );
    let new_tags: Vec<String> = if patch.remove_tags.is_empty() {
        with_added
    } else {
        with_added
            .into_iter()
            .filter(|tag| !patch.remove_tags.contains(tag))
            .collect()
    };
    let tags = (new_tags != current_tags).then(|| dedup(&new_tags));

    let current = current_files(tasks, task.id);
    let new_files = add_files(
        patch.files.clone().unwrap_or_else(|| current.clone()),
        &patch.add_files,
    );
    let files = (new_files != current).then(|| file_rows(task.id, &new_files));

    let project_change = patch.projects.as_ref().and_then(|wanted| {
        let before: Vec<Id> = tasks
            .task_projects
            .values()
            .filter(|link| link.task_id == task.id)
            .map(|link| link.project_id)
            .collect::<BTreeSet<Id>>()
            .into_iter()
            .collect();
        let after: Vec<Id> = dedup(wanted)
            .into_iter()
            .collect::<BTreeSet<Id>>()
            .into_iter()
            .collect();
        (before != after).then_some((before, after))
    });

    let parent_id = if patch.clear_parent && task.parent_id.is_some() {
        None
    } else if let Some(parent) = patch.parent {
        check_set_parent(tasks, task.id, parent)?;
        Some(parent)
    } else {
        task.parent_id
    };

    let new_hash = content_hash(&title, &description);
    let material_change = new_hash != task.description_hash;

    // The change record, in the order the fields are listed.
    let log: Vec<LogEntry> = [
        diff("title", &task.title, &title, text),
        diff("description", &task.description, &description, text),
        (criteria_before != criteria_after).then(|| {
            (
                "acceptance_criteria",
                json!(criteria_before),
                json!(criteria_after),
            )
        }),
        diff("assignee", &task.assignee, &assignee, opt_text),
        diff(
            "assigned_agent",
            &task.assigned_agent,
            &assigned_agent,
            opt_text,
        ),
        diff("auto_execute", &task.auto_execute, &auto_execute, flag),
        diff("due_at", &task.due_at, &due_at, timestamp_json),
        diff(
            "scheduled_at",
            &task.scheduled_at,
            &scheduled_at,
            timestamp_json,
        ),
        diff("priority", &task.priority, &priority, priority_json),
        diff(
            "estimated_effort",
            &task.estimated_effort,
            &estimated_effort,
            opt_object,
        ),
        diff("metadata", &task.metadata, &metadata, object),
        tags.as_ref()
            .map(|_| ("tags", json!(current_tags), json!(new_tags))),
        files.as_ref().map(|_| {
            (
                "files",
                Value::Array(current.iter().map(FileInput::to_json).collect()),
                Value::Array(new_files.iter().map(FileInput::to_json).collect()),
            )
        }),
        project_change
            .as_ref()
            .map(|(before, after)| ("project_ids", ids_json(before), ids_json(after))),
        diff("parent_id", &task.parent_id, &parent_id, parent_json),
    ]
    .into_iter()
    .flatten()
    .collect();
    let projects = project_change.map(|(_, after)| after);

    let closed = [TaskStatus::Accepted, TaskStatus::Cancelled];
    let status_touched = patch.status.is_some_and(|target| {
        target != task.status && (closed.contains(&target) || closed.contains(&task.status))
    });
    let touched: BTreeSet<&str> = log
        .iter()
        .map(|(field, _, _)| {
            if *field == "parent_id" {
                "parent"
            } else {
                *field
            }
        })
        .chain(status_touched.then_some("status"))
        .collect();
    guard_tracker_owned(&stamp.actor, task, &touched, tracked_by(tasks, task.id))?;

    if patch.status == Some(TaskStatus::Claimed) {
        return Err(WgError::validation("use task_claim to claim a task"));
    }

    let changes = log
        .into_iter()
        .fold(Changes::new(), |acc, (field, before, after)| {
            acc.with(field, before, after)
        });
    let changed = !changes.is_empty();
    let next = Task {
        title,
        description,
        acceptance_criteria: criteria_after.iter().cloned().collect(),
        assignee,
        assigned_agent,
        auto_execute,
        due_at,
        scheduled_at,
        priority,
        estimated_effort,
        metadata,
        parent_id,
        description_hash: if material_change {
            new_hash
        } else {
            task.description_hash.clone()
        },
        version: if changed {
            task.version + 1
        } else {
            task.version
        },
        updated_at: if changed { stamp.at } else { task.updated_at },
        ..task.clone()
    };
    let event = changed.then(|| TaskUpdated {
        key: task.key.clone(),
        changes: changes.clone(),
        material_change,
        reason: patch.reason.clone(),
        version: next.version,
    });

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

/// One recorded change: the field, and its value before and after.
type LogEntry = (&'static str, Value, Value);

/// The patched value when the patch gives one, else the stored one.
fn pick<T: Clone>(patch: &Option<T>, current: &T) -> T {
    patch.clone().unwrap_or_else(|| current.clone())
}

/// The `[before, after]` record of a field, when the two differ.
fn diff<T: PartialEq>(
    field: &'static str,
    old: &T,
    new: &T,
    encode: fn(&T) -> Value,
) -> Option<LogEntry> {
    (old != new).then(|| (field, encode(old), encode(new)))
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
    values
        .iter()
        .fold(Vec::with_capacity(values.len()), |out, value| {
            if out.contains(value) {
                out
            } else {
                out.into_iter().chain([value.clone()]).collect()
            }
        })
}
