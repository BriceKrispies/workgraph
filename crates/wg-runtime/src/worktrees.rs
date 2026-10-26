//! The worktree service's shell (WG-3773): crash recovery of Workgraph's managed worktrees, their
//! listing, and the release and removal of an attempt's worktrees, as `WorktreeManager` in
//! `integrations/git/worktrees.py` runs them.
//!
//! The decisions are `wg-runtimespec`'s `worktrees` module. This file reads the rows and the
//! attempts, tasks and stacks they belong to, checks the checkouts and the worktree directory on
//! disk, runs git in the checkouts, and applies each decision in one transaction.

use std::fs;
use std::path::{Path, PathBuf};

use wg_git::{Git, GitError, RunOptions, resolve};
use wg_json::Value;
use wg_model::entity::{ManagedWorktree, RepositoryCheckout, Task};
use wg_model::write::RowKey;
use wg_registry::boundary::is_within;
use wg_runtimespec::RequestContext;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_runtimespec::worktrees::{
    Candidate, RecoveryInput, RecoveryReport, RecoveryRow, ReleaseTarget, Removal, path_key,
    recovery_decision, release_decision, removed_decision, stack_of, worktree_list_out,
};
use wg_store::Session;
use wg_types::{ErrorCode, Id, Permission, StackStatus, TaskStatus, WgError, WorktreeStatus};

use crate::app::App;

/// The directory name under the worktrees root that holds stack worktrees.
const STACKS_DIR: &str = "stacks";

/// The file a worktree directory's parent keeps for the manifest; it does not keep a parent alive.
const MANIFEST_FILE: &str = "workgraph-manifest.json";

/// How much of git's refusal `remove` reports (`stderr.strip()[:300]`).
const REFUSAL_CHARS: usize = 300;

impl App {
    /// `WorktreeManager.recover`: reconciles the managed worktree rows with the disk and their
    /// attempts, and reports the directories under the worktrees root that no row records.
    /// `worker recover` runs it as the system (`RequestContext.system("cli")`).
    pub fn worktree_recover(
        &mut self,
        context: &RequestContext,
    ) -> Result<RecoveryReport, WgError> {
        let root = PathBuf::from(self.paths.worktrees_dir().as_str());
        Ok(self
            .runtime
            .decide(
                context,
                |session| recovery_input(session, &root),
                |input, stamp| Ok(recovery_decision(input, stamp)),
            )?
            .reply)
    }

    /// `WorktreeManager.list`: the managed worktrees, newest first, of one workspace and status
    /// when given. A status that is not a worktree status matches nothing.
    pub fn worktree_list(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        status: Option<&str>,
    ) -> Result<Value, WgError> {
        self.runtime
            .stamps
            .stamp(context)?
            .require(&[Permission::RepoRead])?;
        let workspace_id = match workspace {
            Some(reference) => Some(self.workspace_id(reference)?),
            None => None,
        };
        let status = match status {
            Some(text) => match WorktreeStatus::parse(text) {
                Some(status) => Some(status),
                None => return Ok(worktree_list_out(&[])),
            },
            None => None,
        };
        let rows = self
            .runtime
            .store
            .read(|session| session.worktrees_with_repository(workspace_id, status))?;
        Ok(worktree_list_out(&rows))
    }

    /// `WorktreeManager.release`: the attempt's leases are dropped and its worktrees are marked
    /// released. With `remove`, each of them is then removed (see [`App::worktree_remove`]), and
    /// the removals are returned.
    pub fn worktree_release(
        &mut self,
        context: &RequestContext,
        attempt_id: Id,
        remove: bool,
        force: bool,
    ) -> Result<Vec<Removal>, WgError> {
        let holder = attempt_id.to_string();
        let targets: Vec<ReleaseTarget> = self
            .runtime
            .decide(
                context,
                |session| {
                    let leases = session.leases_held_by(&holder)?;
                    let rows = session.worktrees_of_attempt(
                        attempt_id,
                        &[
                            WorktreeStatus::Active,
                            WorktreeStatus::Preparing,
                            WorktreeStatus::Released,
                        ],
                    )?;
                    Ok((leases, rows))
                },
                |(leases, rows), _stamp| Ok(release_decision(leases, rows)),
            )?
            .reply;
        if !remove {
            return Ok(Vec::new());
        }
        targets
            .iter()
            .map(|target| self.worktree_remove(context, target.id, force))
            .collect()
    }

    /// `WorktreeManager.remove`: removes one managed worktree from disk and git, keeping its
    /// branch, and marks the row removed. A worktree with uncommitted changes is kept unless
    /// `force` is set. Only worktrees under Workgraph's worktrees directory are ever removed.
    pub fn worktree_remove(
        &mut self,
        context: &RequestContext,
        worktree_id: Id,
        force: bool,
    ) -> Result<Removal, WgError> {
        let root_text = self.paths.worktrees_dir().as_str().to_owned();
        let (row, source) = self
            .runtime
            .store
            .read(|session| removal_target(session, worktree_id))?
            .ok_or_else(|| not_found(worktree_id))?;
        if row.status == WorktreeStatus::Removed {
            return Ok(Removal::refused(worktree_id, "already removed"));
        }
        if !is_within(&row.path, &root_text) {
            return Err(WgError::new(
                ErrorCode::ValidationFailed,
                format!(
                    "refusing to remove {}: not inside Workgraph's worktree directory",
                    row.path
                ),
            ));
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
                    let reason: String = result.stderr.trim().chars().take(REFUSAL_CHARS).collect();
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
        self.runtime.decide(
            context,
            |session| {
                session
                    .fetch::<ManagedWorktree>(&RowKey::ManagedWorktree(worktree_id))?
                    .ok_or_else(|| not_found(worktree_id))
            },
            |row, stamp| Ok(removed_decision(row, stamp)),
        )?;
        remove_empty_attempt_dir(&path, Path::new(&root_text));
        Ok(Removal::removed(worktree_id))
    }
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

/// `NotFoundError` for a worktree id.
fn not_found(worktree_id: Id) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("managed worktree {worktree_id} not found"),
    )
}

/// A git failure that could not run at all, as a `GitError` of the API.
fn git_failure(err: GitError) -> WgError {
    WgError::new(ErrorCode::GitError, err.to_string())
}

/// A file-system failure on `path`.
fn io_failure(path: &Path, err: &std::io::Error) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("cannot remove {}: {err}", path.display()),
    )
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

/// The facts recovery decides from, read in the decision's transaction.
fn recovery_input(session: &mut Session<'_>, root: &Path) -> Result<RecoveryInput, WgError> {
    let stacks_root = resolve(&root.join(STACKS_DIR)).display().to_string();
    let open =
        session.worktrees_with_status(&[WorktreeStatus::Preparing, WorktreeStatus::Active])?;
    let mut rows = Vec::with_capacity(open.len());
    for row in open {
        let exists_on_disk = Path::new(&row.path).exists()
            && Git {
                path: PathBuf::from(&row.path),
                timeout_ms: GIT_TIMEOUT_MS,
            }
            .is_repository()
            .unwrap_or(false);
        let attempt = match row.attempt_id {
            Some(attempt_id) => session.attempt_by_id(attempt_id)?,
            None => None,
        };
        let task_closed = match attempt.as_ref() {
            Some(attempt) => session
                .fetch::<Task>(&RowKey::Task(attempt.task_id))?
                .is_some_and(|task| {
                    matches!(task.status, TaskStatus::Accepted | TaskStatus::Cancelled)
                        || task.archived_at.is_some()
                }),
            None => false,
        };
        let stack_ended = match stack_of(&resolved_text(&row.path), &stacks_root) {
            Some(stack_dir) => match Id::parse_str(&stack_dir) {
                Ok(stack_id) => session.stack_by_id(stack_id)?.is_some_and(|stack| {
                    matches!(stack.status, StackStatus::Landed | StackStatus::Abandoned)
                }),
                Err(_) => false,
            },
            None => false,
        };
        rows.push(RecoveryRow {
            attempt_status: attempt.map(|attempt| attempt.status),
            row,
            exists_on_disk,
            task_closed,
            stack_ended,
        });
    }
    let known = session
        .worktrees_with_status(&[])?
        .iter()
        .map(|row| path_key(&resolved_text(&row.path)))
        .collect();
    Ok(RecoveryInput {
        rows,
        known,
        candidates: candidates(root),
    })
}

/// Every directory under each attempt directory of the worktrees root (the stacks directory
/// excluded), in name order. Python's `iterdir` order is the file system's; names are sorted here.
fn candidates(root: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    for attempt_dir in sorted_children(root) {
        let is_stacks = attempt_dir
            .file_name()
            .is_some_and(|name| name == STACKS_DIR);
        if !attempt_dir.is_dir() || is_stacks {
            continue;
        }
        for child in sorted_children(&attempt_dir) {
            if child.is_dir() {
                found.push(Candidate {
                    key: path_key(&resolve(&child).display().to_string()),
                    path: child.display().to_string(),
                });
            }
        }
    }
    found
}

/// The entries of a directory, sorted by name; empty when it cannot be read.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

/// A stored path resolved on disk, as the text recovery compares.
fn resolved_text(stored: &str) -> String {
    resolve(Path::new(stored)).display().to_string()
}
