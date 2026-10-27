//! Administering a repository registration: update, remove, remove a checkout and declare or
//! drop a dependency (`update`, `remove`, `remove_checkout`, `add_dependency` and
//! `remove_dependency` in `repositories.py`).
//!
//! The shell resolves the references (`resolve::repository`) and loads the rows each decision
//! reads. Every decision replies with the id of the repository it changed; the shell reads the
//! repository back for the answer. Ids derive from the stamp by fixed slots.

use std::iter::once;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::event::{
    RepositoryCheckoutRemoved, RepositoryDependencyDeclared, RepositoryRemoved, RepositoryUpdated,
};
use wg_model::{
    Changes, Decision, EventDraft, EventPayload, Repository, RepositoryCheckout,
    RepositoryDependency, Row, RowKey, Stamp, Write, py_repr_str,
};
use wg_types::{AccessMode, Id, Permission, WgError};

use crate::checkout::KIND_MANAGED_WORKTREE;
use crate::register::Matched;

/// Id slots: the event, then the dependency row.
const EVENT_SLOT: u32 = 0;
const DEPENDENCY_SLOT: u32 = 1;

/// The fields of `RepositoryUpdate` that are set. `tracked_branches` replaces the whole list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub access_mode: Option<AccessMode>,
    pub default_branch: Option<String>,
    pub tracked_branches: Option<Vec<String>>,
}

fn event(stamp: &Stamp, repo: &Repository, payload: EventPayload) -> EventDraft {
    EventDraft::new(
        stamp,
        stamp.id(EVENT_SLOT),
        Some(repo.workspace_id),
        repo.id,
        payload,
    )
}

fn branches(items: &VectorSync<String>) -> Vec<String> {
    items.iter().cloned().collect()
}

/// The tracked branches an update asks for: a changed default branch is always tracked, so the
/// sync never misses it. `None` when the update does not touch the list.
fn tracked_after(repo: &Repository, patch: &Patch) -> Option<Vec<String>> {
    let new_default = patch
        .default_branch
        .as_deref()
        .filter(|branch| !branch.is_empty() && Some(*branch) != repo.default_branch.as_deref());
    match new_default {
        None => patch.tracked_branches.clone(),
        Some(branch) => {
            let base = patch
                .tracked_branches
                .clone()
                .unwrap_or_else(|| branches(&repo.tracked_branches));
            Some(if base.iter().any(|b| b == branch) {
                base
            } else {
                base.into_iter().chain(once(branch.to_owned())).collect()
            })
        }
    }
}

/// `changes` with `field: [before, after]` added when `after` is given and differs.
fn compared<T: PartialEq>(
    changes: Changes,
    field: &str,
    before: &T,
    after: Option<&T>,
    json_of: impl Fn(&T) -> Value,
) -> Changes {
    match after.filter(|value| *value != before) {
        Some(value) => changes.with(field, json_of(before), json_of(value)),
        None => changes,
    }
}

/// `RepositoryService.update`: sets the given fields and records `repository.updated` with the
/// ones that changed (nothing is written when none did). Errors: `permission_denied`; `conflict`
/// when the new name is taken.
pub fn update(
    stamp: &Stamp,
    repo: &Repository,
    taken_names: &[String],
    patch: &Patch,
) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    if let Some(name) = patch.name.as_deref()
        && name != repo.name
        && taken_names.iter().any(|taken| taken == name)
    {
        return Err(WgError::conflict(format!(
            "repository name {} is already used in this workspace",
            py_repr_str(name)
        )));
    }
    let tracked = tracked_after(repo, patch);
    let string = |value: &String| json!(value);
    let optional = |value: &Option<String>| json!(value);
    let changes = compared(
        Changes::new(),
        "name",
        &repo.name,
        patch.name.as_ref(),
        string,
    );
    let changes = compared(
        changes,
        "description",
        &repo.description,
        patch.description.as_ref(),
        string,
    );
    let changes = compared(
        changes,
        "access_mode",
        &repo.access_mode,
        patch.access_mode.as_ref(),
        |mode| json!(mode.as_str()),
    );
    let changes = compared(
        changes,
        "default_branch",
        &repo.default_branch,
        patch.default_branch.clone().map(Some).as_ref(),
        optional,
    );
    let changes = compared(
        changes,
        "tracked_branches",
        &branches(&repo.tracked_branches),
        tracked.as_ref(),
        |list| json!(list),
    );
    let reply = Decision::new(repo.id);
    if changes.is_empty() {
        return Ok(reply);
    }
    let row = Repository {
        name: patch.name.clone().unwrap_or_else(|| repo.name.clone()),
        description: patch
            .description
            .clone()
            .unwrap_or_else(|| repo.description.clone()),
        access_mode: patch.access_mode.unwrap_or(repo.access_mode),
        default_branch: patch
            .default_branch
            .clone()
            .or_else(|| repo.default_branch.clone()),
        tracked_branches: tracked
            .map(|list| list.into_iter().collect())
            .unwrap_or_else(|| repo.tracked_branches.clone()),
        updated_at: stamp.at,
        ..repo.clone()
    };
    Ok(reply
        .write(Write::update(Row::Repository(row)))
        .event(event(
            stamp,
            repo,
            EventPayload::RepositoryUpdated(RepositoryUpdated { changes }),
        )))
}

/// `RepositoryService.remove`: archives the registration; git data and history are untouched.
pub fn remove(stamp: &Stamp, repo: &Repository) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    let row = Repository {
        archived_at: Some(stamp.at),
        updated_at: stamp.at,
        ..repo.clone()
    };
    let payload = RepositoryRemoved {
        name: repo.name.clone(),
        note: "registration archived; files on disk untouched".to_owned(),
    };
    Ok(Decision::new(repo.id)
        .write(Write::update(Row::Repository(row)))
        .event(event(stamp, repo, EventPayload::RepositoryRemoved(payload))))
}

/// `RepositoryService.remove_checkout`: forgets the checkout at `path` (or at `resolved`, the
/// same path resolved by the shell). The next user checkout becomes primary when the removed one
/// was. Errors: `not_found` for a path that is not registered; `validation_failed` for a managed
/// worktree.
pub fn remove_checkout(
    stamp: &Stamp,
    matched: &Matched,
    path: &str,
    resolved: &str,
) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    let repo = &matched.repository;
    let Some(target) = matched
        .checkouts
        .iter()
        .find(|c| c.path == path || c.path == resolved)
    else {
        return Err(WgError::not_found(format!(
            "checkout {path} is not registered for {}",
            repo.name
        )));
    };
    if target.kind == KIND_MANAGED_WORKTREE {
        return Err(WgError::validation(
            "managed worktrees are removed through the worktree manager",
        ));
    }
    let promoted: Option<RepositoryCheckout> = matched
        .checkouts
        .iter()
        .find(|c| c.id != target.id && c.kind != KIND_MANAGED_WORKTREE)
        .filter(|_| target.is_primary)
        .map(|next| RepositoryCheckout {
            is_primary: true,
            updated_at: stamp.at,
            ..next.clone()
        });
    let payload = RepositoryCheckoutRemoved {
        path: target.path.clone(),
    };
    let removal =
        Decision::new(repo.id).write(Write::Delete(RowKey::RepositoryCheckout(target.id)));
    let promoted_write = promoted.map(|row| Write::update(Row::RepositoryCheckout(row)));
    Ok(promoted_write
        .into_iter()
        .fold(removal, Decision::write)
        .event(event(
            stamp,
            repo,
            EventPayload::RepositoryCheckoutRemoved(payload),
        )))
}

/// `RepositoryService.add_dependency`: declares that `repo` depends on `other`; declaring it
/// again changes nothing. Errors: `validation_failed` for a dependency on itself.
pub fn add_dependency(
    stamp: &Stamp,
    repo: &Repository,
    other: &Repository,
    existing: Option<&RepositoryDependency>,
    description: &str,
) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    if repo.id == other.id {
        return Err(WgError::validation("a repository cannot depend on itself"));
    }
    if existing.is_some() {
        return Ok(Decision::new(repo.id));
    }
    let row = RepositoryDependency {
        id: stamp.id(DEPENDENCY_SLOT),
        from_repository_id: repo.id,
        to_repository_id: other.id,
        kind: "depends_on".to_owned(),
        description: description.to_owned(),
        created_at: stamp.at,
    };
    let payload = RepositoryDependencyDeclared {
        depends_on: other.id,
        depends_on_name: other.name.clone(),
    };
    Ok(Decision::new(repo.id)
        .write(Write::insert(Row::RepositoryDependency(row)))
        .event(event(
            stamp,
            repo,
            EventPayload::RepositoryDependencyDeclared(payload),
        )))
}

/// `RepositoryService.remove_dependency`: drops the declared dependency, if there is one.
pub fn remove_dependency(
    stamp: &Stamp,
    repo: &Repository,
    existing: Option<&RepositoryDependency>,
) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    let reply = Decision::new(repo.id);
    Ok(match existing {
        Some(row) => reply.write(Write::Delete(RowKey::RepositoryDependency(row.id))),
        None => reply,
    })
}
