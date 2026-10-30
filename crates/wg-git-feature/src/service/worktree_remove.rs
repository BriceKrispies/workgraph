//! `WorktreeManager.remove`: the one implementation of removing a managed worktree, shared by the
//! exec feature's worktree service and the stacks feature's bisect worktrees.

use std::fs;
use std::path::{Path, PathBuf};

use crate::spec::worktrees::{Removal, git_failure, io_failure, removed_decision};
use wg_git::{Git, RunOptions};
use wg_model::entity::{ManagedWorktree, RepositoryCheckout};
use wg_model::write::RowKey;
use wg_registry::boundary::is_within;
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_runtime::spec::effects::GIT_TIMEOUT_MS;
use wg_store::Session;
use wg_types::pyval::prefix;
use wg_types::{Id, WgError, WorktreeStatus};

/// The file an attempt directory keeps its manifest in.
pub const MANIFEST_FILE: &str = "workgraph-manifest.json";

const REFUSAL_CHARS: usize = 300;

/// `WorktreeManager.remove`: removes one managed worktree from disk and git, keeping its
/// branch, and marks the row removed. A worktree with uncommitted changes is kept unless
/// `force` is set. Only worktrees under Workgraph's worktrees directory are ever removed.
pub fn worktree_remove(
    app: &mut App,
    context: &RequestContext,
    worktree_id: Id,
    force: bool,
) -> Result<Removal, WgError> {
    let root_text = app.paths.worktrees_dir().as_str().to_owned();
    let (row, source) = app
        .runtime
        .store
        .read(|session| removal_target(session, worktree_id))?
        .ok_or_else(|| WgError::not_found(format!("managed worktree {worktree_id} not found")))?;
    if row.status == WorktreeStatus::Removed {
        return Ok(Removal::refused(worktree_id, "already removed"));
    }
    if !is_within(&row.path, &root_text) {
        return Err(WgError::validation(format!(
            "refusing to remove {}: not inside Workgraph's worktree directory",
            row.path
        )));
    }
    let path = PathBuf::from(&row.path);
    // The checkout the worktree was made from, when it is still a repository.
    let source_git = source
        .map(|path| Git {
            path: PathBuf::from(path),
            timeout_ms: GIT_TIMEOUT_MS,
        })
        .filter(|git| git.is_repository().unwrap_or(false));
    if path.exists() {
        let worktree = Git {
            path: path.clone(),
            timeout_ms: GIT_TIMEOUT_MS,
        };
        if worktree.is_repository().unwrap_or(false)
            && !force
            && !worktree.is_clean().unwrap_or(false)
        {
            return Ok(Removal::refused(
                worktree_id,
                "worktree has uncommitted changes",
            ));
        }
        if let Some(source_git) = &source_git {
            let result = source_git
                .worktree_remove(&row.path, force, RunOptions::UNCHECKED)
                .map_err(git_failure)?;
            if result.returncode != 0 && path.exists() {
                let reason: String = prefix(result.stderr.trim(), REFUSAL_CHARS);
                return Ok(Removal::refused(worktree_id, reason));
            }
        } else if force && path.exists() {
            fs::remove_dir_all(&path).map_err(|err| io_failure(&path, &err))?;
        }
    }
    if let Some(source_git) = &source_git {
        source_git
            .worktree_prune(RunOptions::UNCHECKED)
            .map_err(git_failure)?;
    }
    app.runtime.decide(
        context,
        |session| {
            session
                .fetch::<ManagedWorktree>(&RowKey::ManagedWorktree(worktree_id))?
                .ok_or_else(|| {
                    WgError::not_found(format!("managed worktree {worktree_id} not found"))
                })
        },
        |row, stamp| Ok(removed_decision(row, stamp)),
    )?;
    remove_empty_attempt_dir(&path, Path::new(&root_text));
    Ok(Removal::removed(worktree_id))
}

/// The row to remove and the path of the checkout it was made from.
fn removal_target(
    session: &mut Session<'_>,
    worktree_id: Id,
) -> Result<Option<(ManagedWorktree, Option<String>)>, WgError> {
    let Some(row) = session.fetch::<ManagedWorktree>(&RowKey::ManagedWorktree(worktree_id))? else {
        return Ok(None);
    };
    let source = session
        .fetch::<RepositoryCheckout>(&RowKey::RepositoryCheckout(row.source_checkout_id))?
        .map(|checkout| checkout.path);
    Ok(Some((row, source)))
}

/// Removes the attempt directory a removed worktree sat in, when nothing else is left in it but the
/// manifest. The worktrees root itself is never removed.
fn remove_empty_attempt_dir(path: &Path, root: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if !parent.exists()
        || parent == root
        || !is_within(&parent.display().to_string(), &root.display().to_string())
    {
        return;
    }
    let leftovers = fs::read_dir(parent).map(|entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_name() != MANIFEST_FILE)
            .count()
    });
    if leftovers.is_ok_and(|count| count == 0) {
        let _ = fs::remove_dir_all(parent);
    }
}
