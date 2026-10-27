//! The integration targets of a stack and the managed worktrees they run in
//! (`StackIntegrator.target`, `ensure_worktree`; `StackVerifier._bisect_worktree`;
//! `WorktreeService.remove`).

use std::path::{Path, PathBuf};

use serde_json::Value;
use wg_model::entity::{ManagedWorktree, RepositoryCheckout};
use wg_model::event::{EventDraft, EventPayload, WorktreeCreated};
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_registry::checkout::primary_checkout;
use wg_stacks::merge::{directory_name, is_inactive};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::{ErrorCode, Id, WgError, WorktreeStatus};

/// One repository of a stack: where its integration branch lives and what it starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationTarget {
    pub stack_id: Id,
    pub workspace_id: Id,
    pub repository_id: Id,
    pub repository_name: String,
    pub integration_branch: String,
    pub base_sha: String,
    pub source_path: String,
    pub checkout_id: Id,
}

/// `StackIntegrator.target`: `None` for a stack that landed or was abandoned.
pub fn integration_target(
    world: &TaskWorld,
    stack_id: Id,
    repository_id: Id,
    checkouts: &[RepositoryCheckout],
) -> Result<Option<IntegrationTarget>, WgError> {
    let missing = || {
        WgError::new(
            ErrorCode::NotFound,
            format!("stack {stack_id} has no repository {repository_id}"),
        )
    };
    let stack = world.stacks.stack(stack_id).ok_or_else(missing)?;
    let link = world
        .stacks
        .repositories_of(stack_id)
        .find(|row| row.repository_id == repository_id)
        .ok_or_else(missing)?;
    let repository = world
        .tasks
        .registry
        .repository(repository_id)
        .ok_or_else(missing)?;
    if is_inactive(stack.status) {
        return Ok(None);
    }
    let checkout = primary_checkout(checkouts, true).ok_or_else(|| {
        WgError::new(
            ErrorCode::GitError,
            format!("repository {} has no available checkout", repository.name),
        )
    })?;
    Ok(Some(IntegrationTarget {
        stack_id,
        workspace_id: stack.workspace_id,
        repository_id,
        repository_name: repository.name.clone(),
        integration_branch: link.integration_branch.clone(),
        base_sha: link.base_sha.clone(),
        source_path: checkout.path.clone(),
        checkout_id: checkout.id,
    }))
}

/// Where a stack's integration worktree for one repository lives (`worktree_path`).
#[must_use]
pub fn integration_path(worktrees_dir: &Path, stack_id: Id, repository: &str) -> PathBuf {
    worktrees_dir
        .join("stacks")
        .join(stack_id.to_string())
        .join(directory_name(repository))
}

/// Where the detached worktree that probes prefixes of one repository lives.
#[must_use]
pub fn bisect_path(worktrees_dir: &Path, stack_id: Id, repository: &str) -> PathBuf {
    worktrees_dir
        .join("stacks")
        .join(stack_id.to_string())
        .join(format!("bisect-{}", directory_name(repository)))
}

/// What a worktree row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The stack's integration branch (`ensure_worktree`).
    Integration,
    /// A detached worktree for probes (`_bisect_worktree`).
    Bisect,
}

/// The row and event that record a worktree as active: a new row, or the existing row of the
/// path brought back. The reply is the row's id.
#[must_use]
pub fn worktree_active(
    existing: Option<&ManagedWorktree>,
    target: &IntegrationTarget,
    path: &str,
    purpose: Purpose,
    stamp: &Stamp,
) -> Decision<Id> {
    let (row_id, ids) = stamp.ids().next();
    let (event_id, _) = ids.next();
    let branch = match purpose {
        Purpose::Integration => target.integration_branch.clone(),
        Purpose::Bisect => String::new(),
    };
    let row = match existing {
        Some(found) => ManagedWorktree {
            status: WorktreeStatus::Active,
            base_sha: match purpose {
                Purpose::Integration => found.base_sha.clone(),
                Purpose::Bisect => target.base_sha.clone(),
            },
            removed_at: match purpose {
                Purpose::Integration => found.removed_at,
                Purpose::Bisect => None,
            },
            ..found.clone()
        },
        None => ManagedWorktree {
            id: row_id,
            workspace_id: target.workspace_id,
            repository_id: target.repository_id,
            source_checkout_id: target.checkout_id,
            attempt_id: None,
            path: path.to_owned(),
            branch: branch.clone(),
            base_sha: target.base_sha.clone(),
            status: WorktreeStatus::Active,
            last_error: None,
            created_at: stamp.at,
            updated_at: stamp.at,
            removed_at: None,
        },
    };
    let write = match existing {
        None => Some(Write::insert(row.clone())),
        Some(found) if *found != row => Some(Write::update(ManagedWorktree {
            updated_at: stamp.at,
            ..row.clone()
        })),
        Some(_) => None,
    };
    let (purpose_text, event_branch) = match purpose {
        Purpose::Integration => ("stack_integration", Some(branch)),
        Purpose::Bisect => ("stack_bisect", None),
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(target.workspace_id),
        row.id,
        EventPayload::WorktreeCreated(WorktreeCreated {
            purpose: Some(purpose_text.to_owned()),
            stack_id: Some(target.stack_id),
            repository: target.repository_name.clone(),
            path: path.to_owned(),
            base_sha: target.base_sha.clone(),
            base: None,
            branch: event_branch,
        }),
    );
    let decision = write
        .into_iter()
        .fold(Decision::new(row.id), Decision::write);
    decision.event(event)
}

/// Whether a stored path is the one the executor computed (both sides as text).
#[must_use]
pub fn same_path(stored: &str, computed: &str) -> bool {
    stored == computed
}

/// `Value` of a path for event data.
#[must_use]
pub fn path_value(path: &Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}
