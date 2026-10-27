//! The repository commands as pure functions (IF-04): the input of a registration and the
//! request the registry decides on, built from what git said about the path.
//!
//! The runtime reads git, calls [`register_request`] and hands the request to
//! `wg_registry::register::register`; `update`, `remove`, `refresh` and the rest take their input
//! straight from `wg_registry::admin` and `wg_registry::refresh`.

use serde_json::Value as Json;
use wg_registry::admin::Patch;
use wg_registry::checkout::{KIND_CLONE, KIND_WORKTREE, WorktreeDetail, WorktreeRecord};
use wg_registry::identity::fingerprint;
use wg_registry::register::Request;
use wg_types::{AccessMode, ErrorCode, Id, WgError};

/// A git call that failed, as the error the services raise (`GitError`), with its message.
#[must_use]
pub fn git_failure(message: String) -> WgError {
    WgError::new(ErrorCode::GitError, message)
}

/// A request the services refuse before any write, with the message the Python service gives.
#[must_use]
pub fn validation(message: String) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// `RepositoryRegister`: what the caller asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterInput {
    pub path: String,
    pub name: Option<String>,
    pub description: String,
    pub read_only: bool,
    pub tracked_branches: Vec<String>,
    pub projects: Vec<String>,
}

/// What git said about the working tree a registration names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// The resolved top of the working tree.
    pub top: String,
    /// The last component of `top`.
    pub dir_name: String,
    pub root_commits: Vec<String>,
    pub remotes: Vec<(String, String)>,
    pub is_worktree: bool,
    pub default_branch: Option<String>,
    pub shallow: bool,
    pub worktrees: Vec<(WorktreeRecord, WorktreeDetail)>,
}

/// The registry's request for `input` in `workspace_id`, with the roots of the workspace's
/// boundary.
pub fn register_request(
    workspace_id: Id,
    input: &RegisterInput,
    facts: GitFacts,
    allowed_roots: Vec<String>,
    managed_roots: Vec<String>,
) -> Request {
    Request {
        workspace_id,
        fingerprint: fingerprint(&facts.root_commits),
        path: facts.top,
        dir_name: facts.dir_name,
        requested_name: input.name.clone(),
        description: input.description.clone(),
        read_only: input.read_only,
        tracked_branches: input.tracked_branches.clone(),
        projects: input.projects.clone(),
        remotes: facts.remotes,
        kind: if facts.is_worktree {
            KIND_WORKTREE
        } else {
            KIND_CLONE
        },
        default_branch: facts.default_branch,
        shallow: facts.shallow,
        worktrees: facts.worktrees,
        allowed_roots,
        managed_roots,
    }
}

/// The text of a body field, `None` when it is absent or `null`.
fn text_field(body: &Json, key: &str) -> Option<String> {
    body.get(key).and_then(Json::as_str).map(str::to_owned)
}

/// The list of strings of a body field, empty when it is absent.
fn strings_field(body: &Json, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The `RepositoryRegister` body (`POST /repositories`) as the registration input: an
/// `access_mode` of `read_only` registers a read-only repository.
#[must_use]
pub fn register_input(body: &Json) -> RegisterInput {
    RegisterInput {
        path: text_field(body, "path").unwrap_or_default(),
        name: text_field(body, "name"),
        description: text_field(body, "description").unwrap_or_default(),
        read_only: body.get("access_mode").and_then(Json::as_str) == Some("read_only"),
        tracked_branches: strings_field(body, "tracked_branches"),
        projects: strings_field(body, "projects"),
    }
}

/// The `RepositoryUpdate` body (`PATCH /repositories/{ref}`) as the patch of the admin decision:
/// the fields that are set (absent or `null` ones are left alone).
pub fn repository_patch(body: &Json) -> Result<Patch, WgError> {
    let access_mode = match body.get("access_mode").and_then(Json::as_str) {
        None => None,
        Some("read_only") => Some(AccessMode::ReadOnly),
        Some("read_write") => Some(AccessMode::ReadWrite),
        Some(other) => {
            return Err(validation(format!(
                "access_mode {other:?} is not one of 'read_only', 'read_write'"
            )));
        }
    };
    Ok(Patch {
        name: text_field(body, "name"),
        description: text_field(body, "description"),
        access_mode,
        default_branch: text_field(body, "default_branch"),
        tracked_branches: body
            .get("tracked_branches")
            .and_then(Json::as_array)
            .map(|_| strings_field(body, "tracked_branches")),
    })
}

/// The `mode` query of `POST /repositories/{ref}/sync`: `true` for a full reconcile. Any other
/// mode is refused, as `GitService.sync` refuses it.
pub fn sync_is_full(mode: &str) -> Result<bool, WgError> {
    match mode {
        "incremental" => Ok(false),
        "full" => Ok(true),
        _ => Err(validation("mode must be incremental or full".to_owned())),
    }
}
