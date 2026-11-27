//! The child limit of a task create (`tasks.max_open_children`).
//!
//! Orchestrators asked to break work down tend to write one task per step, so a parent ends up
//! with hundreds of children that are really the checklist of a few tasks. With the limit set, a
//! create under a parent that already has that many open children is refused, and the refusal
//! lists them: the new work belongs inside one of them (a criterion or a comment), or under a new
//! milestone parent. Zero, the default, turns the check off.

use serde_json::{Map, Value, json};
use wg_model::slice::TaskSlice;
use wg_types::{Id, TaskStatus, WgError};

use crate::duplicates::is_open;

/// Reads `tasks.max_open_children`; zero, absent or not a non-negative integer is off.
#[must_use]
pub fn child_limit(config: &Map<String, Value>) -> Option<usize> {
    config
        .get("tasks")
        .and_then(|tasks| tasks.get("max_open_children"))
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .filter(|limit| *limit > 0)
}

/// An open child of a parent: its key, title and status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChild {
    pub key: String,
    pub title: String,
    pub status: TaskStatus,
}

/// The open children of `parent` (not accepted, cancelled or archived), by key.
#[must_use]
pub fn open_children(tasks: &TaskSlice, parent: Id) -> Vec<OpenChild> {
    let mut found: Vec<OpenChild> = tasks
        .tasks
        .values()
        .filter(|task| task.parent_id == Some(parent) && is_open(task))
        .map(|task| OpenChild {
            key: task.key.to_string(),
            title: task.title.clone(),
            status: task.status,
        })
        .collect();
    found.sort_by(|a, b| a.key.cmp(&b.key));
    found
}

/// Refuses a create under `parent_key` when its open children already reach `limit`.
pub fn refuse_over_limit(
    parent_key: &str,
    children: &[OpenChild],
    limit: usize,
) -> Result<(), WgError> {
    if children.len() < limit {
        return Ok(());
    }
    let lines = children
        .iter()
        .map(|c| format!("  {} \"{}\" [{}]", c.key, c.title, c.status))
        .collect::<Vec<_>>()
        .join("\n");
    Err(WgError::validation(format!(
        "{parent_key} already has {} open children (tasks.max_open_children is {limit}):\n{lines}\n\
         A task is a milestone that leaves the system working in a new, checkable state, not a step. \
         Add this work to one of these tasks as an acceptance criterion or a comment, finish or \
         cancel one of them first, or put it under a new milestone parent",
        children.len()
    ))
    .detail("reason", "too_many_children")
    .detail("parent", json!(parent_key))
    .detail("limit", json!(limit))
    .detail(
        "children",
        Value::Array(
            children
                .iter()
                .map(|c| json!({"key": c.key, "title": c.title, "status": c.status.as_str()}))
                .collect(),
        ),
    ))
}
