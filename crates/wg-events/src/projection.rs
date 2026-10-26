//! The projection of task state from the event log (`core/events/projection.py`): replaying the
//! task events, in sequence order, reproduces the task state that the current-state tables hold.
//! `events verify` compares the two (see [`crate::verify`]).
//!
//! The projection reads the fields the Python projection reads. Its own payloads are the upcast
//! records of [`crate::upcast::read`]; the event payloads it does not read are ignored.

use rpds::RedBlackTreeMapSync;
use serde_json::{Value, json};
use wg_model::{JsonObject, StoredEvent};
use wg_pynum::stable_sort_by;
use wg_types::Id;

/// The projected state of one task. Scalars are kept as JSON values so that a payload's `null`
/// compares as Python's `None` does; list fields hold their string items.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedTask {
    pub id: Id,
    pub key: Value,
    pub title: Value,
    pub description: Value,
    pub status: Value,
    pub priority: Value,
    pub tags: Vec<String>,
    pub project_ids: Vec<String>,
    pub repository_ids: Vec<String>,
    pub parent_id: Value,
    pub depends_on: Vec<String>,
    pub archived: bool,
}

impl ProjectedTask {
    /// `ProjectedTask.comparable()`: the fields `events verify` compares, lists sorted.
    #[must_use]
    pub fn comparable(&self) -> JsonObject {
        [
            ("key", self.key.clone()),
            ("title", self.title.clone()),
            ("description", self.description.clone()),
            ("status", self.status.clone()),
            ("priority", self.priority.clone()),
            ("tags", sorted_list(&self.tags)),
            ("project_ids", sorted_list(&self.project_ids)),
            ("repository_ids", sorted_list(&self.repository_ids)),
            ("parent_id", self.parent_id.clone()),
            ("depends_on", sorted_list(&self.depends_on)),
            ("archived", json!(self.archived)),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
    }
}

fn sorted_list(items: &[String]) -> Value {
    Value::Array(
        stable_sort_by(items.iter().cloned(), |a, b| a.cmp(b))
            .into_iter()
            .map(Value::String)
            .collect(),
    )
}

/// A projection failure: a payload lacks a field the projection reads, or has the wrong shape.
/// Python raises here (`KeyError`, `ValueError`); the verify report cannot be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    MissingField {
        seq: i64,
        event_type: String,
        field: String,
    },
    Malformed {
        seq: i64,
        event_type: String,
        detail: String,
    },
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField {
                seq,
                event_type,
                field,
            } => write!(f, "event {seq} ({event_type}) has no `{field}`"),
            Self::Malformed {
                seq,
                event_type,
                detail,
            } => write!(f, "event {seq} ({event_type}) is malformed: {detail}"),
        }
    }
}

impl std::error::Error for ProjectionError {}

/// The projected tasks by entity id.
pub type TaskMap = RedBlackTreeMapSync<Id, ProjectedTask>;

fn missing(event: &StoredEvent, field: &str) -> ProjectionError {
    ProjectionError::MissingField {
        seq: event.seq,
        event_type: event.event_type.clone(),
        field: field.to_owned(),
    }
}

fn malformed(event: &StoredEvent, detail: &str) -> ProjectionError {
    ProjectionError::Malformed {
        seq: event.seq,
        event_type: event.event_type.clone(),
        detail: detail.to_owned(),
    }
}

/// `data[name]`, which must be present.
fn field<'a>(event: &'a StoredEvent, name: &str) -> Result<&'a Value, ProjectionError> {
    event.data.get(name).ok_or_else(|| missing(event, name))
}

/// `data[name]` as a string, which the projection needs for ids.
fn text_field(event: &StoredEvent, name: &str) -> Result<String, ProjectionError> {
    field(event, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| malformed(event, &format!("`{name}` is not a string")))
}

/// `list(value)` for a list of ids: its string items (a non-list reads as empty).
fn id_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `ProjectedTask(...)` from a `task.created` payload, with the defaults Python gives.
fn created(event: &StoredEvent) -> Result<ProjectedTask, ProjectionError> {
    let data = &event.data;
    let or = |name: &str, default: Value| data.get(name).cloned().unwrap_or(default);
    let list = |name: &str| data.get(name).map(id_list).unwrap_or_default();
    Ok(ProjectedTask {
        id: event.entity_id,
        key: field(event, "key")?.clone(),
        title: or("title", json!("")),
        description: or("description", json!("")),
        status: or("status", json!("backlog")),
        priority: or("priority", json!("medium")),
        tags: list("tags"),
        project_ids: list("project_ids"),
        repository_ids: list("repository_ids"),
        parent_id: or("parent_id", Value::Null),
        depends_on: list("depends_on"),
        archived: false,
    })
}

/// `setattr(task, attr, new)` for a field of `task.updated`'s `changes`; other fields are not
/// projected. A list field takes the list's items; a non-list leaves it empty.
fn changed(task: ProjectedTask, name: &str, new: &Value) -> ProjectedTask {
    match name {
        "title" => ProjectedTask {
            title: new.clone(),
            ..task
        },
        "description" => ProjectedTask {
            description: new.clone(),
            ..task
        },
        "priority" => ProjectedTask {
            priority: new.clone(),
            ..task
        },
        "parent_id" => ProjectedTask {
            parent_id: new.clone(),
            ..task
        },
        "tags" => ProjectedTask {
            tags: id_list(new),
            ..task
        },
        "project_ids" => ProjectedTask {
            project_ids: id_list(new),
            ..task
        },
        _ => task,
    }
}

/// `task.updated`: each `changes` entry is `name: [old, new]`.
fn updated(task: ProjectedTask, event: &StoredEvent) -> Result<ProjectedTask, ProjectionError> {
    match event.data.get("changes") {
        None => Ok(task),
        Some(Value::Object(changes)) => changes.iter().try_fold(task, |task, (name, pair)| {
            match pair.as_array().map(Vec::as_slice) {
                Some([_, new]) => Ok(changed(task, name, new)),
                _ => Err(malformed(
                    event,
                    &format!("change `{name}` is not [old, new]"),
                )),
            }
        }),
        Some(_) => Err(malformed(event, "`changes` is not an object")),
    }
}

fn with_item(items: &[String], item: String) -> Vec<String> {
    if items.contains(&item) {
        items.to_vec()
    } else {
        items.iter().cloned().chain(std::iter::once(item)).collect()
    }
}

fn without_item(items: &[String], item: &str) -> Vec<String> {
    items
        .iter()
        .filter(|kept| kept.as_str() != item)
        .cloned()
        .collect()
}

/// One event applied to one task (the `match` of `project_tasks`).
fn step(task: ProjectedTask, event: &StoredEvent) -> Result<ProjectedTask, ProjectionError> {
    let next = match event.event_type.as_str() {
        "task.updated" => return updated(task, event),
        "task.status_changed" => ProjectedTask {
            status: field(event, "to")?.clone(),
            ..task
        },
        "task.dependency_added" => {
            let dependency = text_field(event, "depends_on")?;
            ProjectedTask {
                depends_on: with_item(&task.depends_on, dependency),
                ..task
            }
        }
        "task.dependency_removed" => {
            let dependency = text_field(event, "depends_on")?;
            ProjectedTask {
                depends_on: without_item(&task.depends_on, &dependency),
                ..task
            }
        }
        "task.repository_linked" => {
            let repository = text_field(event, "repository_id")?;
            ProjectedTask {
                repository_ids: with_item(&task.repository_ids, repository),
                ..task
            }
        }
        "task.repository_unlinked" => {
            let repository = text_field(event, "repository_id")?;
            ProjectedTask {
                repository_ids: without_item(&task.repository_ids, &repository),
                ..task
            }
        }
        "task.archived" => ProjectedTask {
            archived: true,
            ..task
        },
        "task.unarchived" => ProjectedTask {
            archived: false,
            ..task
        },
        _ => task,
    };
    Ok(next)
}

fn apply(tasks: TaskMap, event: &StoredEvent) -> Result<TaskMap, ProjectionError> {
    if event.entity_type != "task" {
        return Ok(tasks);
    }
    if event.event_type == "task.created" {
        return Ok(tasks.insert(event.entity_id, created(event)?));
    }
    match tasks.get(&event.entity_id).cloned() {
        None => Ok(tasks),
        Some(task) => {
            let next = step(task, event)?;
            Ok(tasks.insert(event.entity_id, next))
        }
    }
}

/// `project_tasks(events)`: replays the task events in sequence order. The events must already be
/// upcast (see [`crate::upcast::read`]). Events of other entity types are skipped, and so are
/// events for a task with no `task.created` before them.
///
/// # Errors
///
/// Returns a [`ProjectionError`] for a payload the projection cannot read.
pub fn project_tasks(events: Vec<StoredEvent>) -> Result<TaskMap, ProjectionError> {
    let ordered = stable_sort_by(events, |a, b| a.seq.cmp(&b.seq));
    ordered.iter().try_fold(TaskMap::new_sync(), apply)
}
