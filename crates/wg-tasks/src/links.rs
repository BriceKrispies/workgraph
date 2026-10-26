//! Task relations and repository links (CORE-04). Mirrors `add_relation`, `link_repository` and
//! `unlink_repository` in `core/services/tasks.py`.
//!
//! The functions decide whether a row is added, updated or left alone, and return the event
//! payloads the shell writes. The shell stores the rows.

use serde_json::{Value, json};
use wg_model::entity::{Repository, TaskRepository};
use wg_model::{Task, TaskSlice};
use wg_types::{AccessMode, ErrorCode, Id, RelationType, WgError};

/// The access values a task link accepts (`link_repository`).
pub const ACCESS_VALUES: &[&str] = &["read_only", "read_write"];

/// What adding a relation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationAdd {
    /// The same relation exists already, with this id; nothing is written.
    Exists(Id),
    /// The relation is new and the shell writes it with a `task.relation_added` event.
    Add,
}

fn missing(kind: &str, id: Id) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("{kind} {id} not found"))
}

/// The decision of `add_relation`: `source` relates to `target` by `relation_type`.
pub fn check_add_relation(
    tasks: &TaskSlice,
    source: Id,
    target: Id,
    relation_type: RelationType,
) -> Result<RelationAdd, WgError> {
    let source_task = tasks.task(source).ok_or_else(|| missing("task", source))?;
    let target_task = tasks.task(target).ok_or_else(|| missing("task", target))?;
    if target_task.workspace_id != source_task.workspace_id {
        return Err(missing("task", target));
    }
    if target == source {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "a task cannot relate to itself",
        ));
    }
    let existing = tasks.relations.values().find(|row| {
        row.source_task_id == source
            && row.target_task_id == target
            && row.relation_type == relation_type
    });
    Ok(existing.map_or(RelationAdd::Add, |row| RelationAdd::Exists(row.id)))
}

/// The `task.relation_added` payload of a new relation to `target`.
pub fn relation_event_data(target: &Task, relation_type: RelationType) -> Value {
    json!({
        "target": target.id.to_string(),
        "target_key": target.key.to_string(),
        "relation_type": relation_type.as_str(),
    })
}

/// What `link_repository` does to the link row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoLinkPlan {
    /// The link is new: insert it with `access`, bump the task version, write
    /// `task.repository_linked` with [`repository_event_data`].
    Insert { access: &'static str },
    /// The link exists with another access: set `access`. No version bump, no event.
    Update { access: &'static str },
    /// The link exists with this access: nothing changes.
    Unchanged,
}

/// Maps an access wire string to its static value.
fn access_value(text: &str) -> Option<&'static str> {
    ACCESS_VALUES.iter().copied().find(|value| *value == text)
}

/// The decision of `link_repository`. `access` is the caller's `read_only`/`read_write` choice,
/// `None` to use the repository's own access mode. `existing` is the current link row, if any.
pub fn plan_link_repository(
    task: &Task,
    repository: &Repository,
    access: Option<&str>,
    existing: Option<&TaskRepository>,
) -> Result<RepoLinkPlan, WgError> {
    if repository.workspace_id != task.workspace_id {
        return Err(missing("repository", repository.id));
    }
    let requested = match access {
        None => None,
        Some(text) => Some(access_value(text).ok_or_else(|| {
            WgError::new(
                ErrorCode::ValidationFailed,
                "access must be read_only or read_write",
            )
        })?),
    };
    let effective = requested.unwrap_or_else(|| repository.access_mode.as_str());
    if effective == AccessMode::ReadWrite.as_str() && repository.access_mode == AccessMode::ReadOnly
    {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("repository {} is registered read-only", repository.name),
        ));
    }
    Ok(match existing {
        None => RepoLinkPlan::Insert { access: effective },
        Some(link) if link.access == effective => RepoLinkPlan::Unchanged,
        Some(_) => RepoLinkPlan::Update { access: effective },
    })
}

/// The `task.repository_linked` payload.
pub fn repository_event_data(repository: &Repository, access: &str) -> Value {
    json!({
        "repository_id": repository.id.to_string(),
        "repository": repository.name,
        "access": access,
    })
}

/// The decision of `unlink_repository`: `linked` is whether the link row existed. Returns the
/// `task.repository_unlinked` payload when the link was removed (the version is bumped then),
/// `None` when there was nothing to remove.
pub fn plan_unlink_repository(repository: &Repository, linked: bool) -> Option<Value> {
    linked.then(|| {
        json!({
            "repository_id": repository.id.to_string(),
            "repository": repository.name,
        })
    })
}
