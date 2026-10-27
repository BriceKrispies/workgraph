//! Resolution of human-friendly references (UUIDs, task keys, slugs and names) to entities.
//!
//! The rules of `src/workgraph/core/services/resolve.py`, as lookups over a loaded slice. A
//! reference that parses as a UUID is tried by id first; a miss (or an id in another workspace)
//! falls through to the name-like lookup, as in Python. Error messages and the `ref` detail are
//! Python's.

use wg_model::{Project, RegistrySlice, Repository, Task, TaskSlice, Workspace, py_repr_str};
use wg_types::{Id, WgError, parse_id};

/// The reference as a UUID, when it is one (Python's `is_uuid`).
fn as_uuid(reference: &str) -> Option<Id> {
    parse_id(reference).ok()
}

/// A workspace by UUID, else by slug.
pub fn workspace<'a>(slice: &'a RegistrySlice, reference: &str) -> Result<&'a Workspace, WgError> {
    as_uuid(reference)
        .and_then(|id| slice.workspace(id))
        .or_else(|| slice.workspace_by_slug(reference))
        .ok_or_else(|| {
            WgError::not_found(format!("workspace {} not found", py_repr_str(reference)))
                .detail("ref", reference)
        })
}

/// A project of the workspace, by UUID, else by slug. A project of another workspace is not found.
pub fn project<'a>(
    slice: &'a RegistrySlice,
    workspace_id: Id,
    reference: &str,
) -> Result<&'a Project, WgError> {
    as_uuid(reference)
        .and_then(|id| slice.project(id))
        .filter(|project| project.workspace_id == workspace_id)
        .or_else(|| {
            slice
                .projects
                .values()
                .find(|project| project.workspace_id == workspace_id && project.slug == reference)
        })
        .ok_or_else(|| {
            WgError::not_found(format!(
                "project {} not found in workspace",
                py_repr_str(reference)
            ))
            .detail("ref", reference)
        })
}

/// A repository of the workspace, by UUID, else by name (not slug). An archived repository is
/// found only when `include_archived` is set.
pub fn repository<'a>(
    slice: &'a RegistrySlice,
    workspace_id: Id,
    reference: &str,
    include_archived: bool,
) -> Result<&'a Repository, WgError> {
    as_uuid(reference)
        .and_then(|id| slice.repository(id))
        .filter(|repo| repo.workspace_id == workspace_id)
        .or_else(|| {
            slice
                .repositories
                .values()
                .find(|repo| repo.workspace_id == workspace_id && repo.name == reference)
        })
        .filter(|repo| include_archived || repo.archived_at.is_none())
        .ok_or_else(|| {
            WgError::not_found(format!(
                "repository {} not found in workspace",
                py_repr_str(reference)
            ))
            .detail("ref", reference)
        })
}

/// A task by UUID, else by key (`WG-104`, matched case-insensitively with surrounding whitespace
/// ignored). With no workspace given, a key held by tasks of more than one workspace is ambiguous.
/// A task outside `workspace_id` is not found.
pub fn task<'a>(
    slice: &'a TaskSlice,
    workspace_id: Option<Id>,
    reference: &str,
) -> Result<&'a Task, WgError> {
    if reference.is_empty() {
        return Err(WgError::validation("task reference must not be empty"));
    }
    let found = match as_uuid(reference).and_then(|id| slice.task(id)) {
        Some(task) => Some(task),
        None => {
            let wanted = reference.trim().to_uppercase();
            // At most two matches are needed to tell "one" from "ambiguous", as in Python's limit(2).
            let matches: Vec<&Task> = slice
                .tasks
                .values()
                .filter(|task| {
                    workspace_id.is_none_or(|ws| task.workspace_id == ws)
                        && task.key.to_string().to_uppercase() == wanted
                })
                .take(2)
                .collect();
            if matches.len() > 1 {
                return Err(WgError::validation(format!(
                    "task key {} is ambiguous across workspaces; pass a workspace",
                    py_repr_str(reference)
                )));
            }
            matches.first().copied()
        }
    };
    match found {
        Some(task) if workspace_id.is_none_or(|ws| task.workspace_id == ws) => Ok(task),
        _ => Err(
            WgError::not_found(format!("task {} not found", py_repr_str(reference)))
                .detail("ref", reference),
        ),
    }
}
