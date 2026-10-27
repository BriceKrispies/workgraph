//! The pure half of `git worktrees` and `git cleanup-worktrees`: the listing rows and the dry-run
//! plan. Ported from `WorktreeManager.list` and `WorktreeManager.cleanup` in
//! `integrations/git/worktrees.py`; the removal itself is the shell's.

use std::path::Path;

use wg_git::GitError;
use wg_json::Value;
use wg_model::decision::Decision;
use wg_model::entity::run::ManagedWorktree;
use wg_model::event::{EventDraft, EventPayload, WorktreeRemoved};
use wg_model::stamp::Stamp;
use wg_model::write::Write;
use wg_types::{ErrorCode, Id, WgError, WorktreeStatus};
use wg_wire::convention::{
    TimeFormat, dict, id, optional_id, optional_text, optional_timestamp, text, timestamp,
};

/// One managed worktree with the name of its repository.
#[derive(Debug, Clone, Copy)]
pub struct WorktreeRow<'a> {
    pub worktree: &'a ManagedWorktree,
    pub repository: &'a str,
}

/// The `worktrees` listing: newest first (the caller orders the rows).
pub fn list_value(rows: &[WorktreeRow<'_>]) -> Value {
    Value::List(rows.iter().map(list_item).collect())
}

fn list_item(row: &WorktreeRow<'_>) -> Value {
    let worktree = row.worktree;
    dict(vec![
        ("id", id(worktree.id)),
        ("repository", text(row.repository)),
        ("repository_id", id(worktree.repository_id)),
        ("attempt_id", optional_id(worktree.attempt_id)),
        ("path", text(&worktree.path)),
        ("branch", text(&worktree.branch)),
        ("base_sha", text(&worktree.base_sha)),
        ("status", text(worktree.status.as_str())),
        ("last_error", optional_text(worktree.last_error.as_deref())),
        (
            "created_at",
            timestamp(worktree.created_at, TimeFormat::Isoformat),
        ),
        (
            "removed_at",
            optional_timestamp(worktree.removed_at, TimeFormat::Isoformat),
        ),
    ])
}

/// The dry-run result of `cleanup-worktrees`: every released worktree with the action a real run
/// would take (`would remove (if clean)`).
pub fn cleanup_dry_run_value(rows: &[WorktreeRow<'_>]) -> Value {
    Value::List(
        rows.iter()
            .map(|row| {
                let mut item = match list_item(row) {
                    Value::Dict(entries) => entries,
                    other => return other,
                };
                item.push((
                    wg_json::PyStr::from("action"),
                    text("would remove (if clean)"),
                ));
                Value::Dict(item)
            })
            .collect(),
    )
}

/// Whether a worktree row is a release candidate of `cleanup-worktrees`.
pub fn is_cleanup_candidate(worktree: &ManagedWorktree) -> bool {
    worktree.status == WorktreeStatus::Released
}

/// The worktree id of a row, for the removal plan.
pub fn worktree_id(worktree: &ManagedWorktree) -> Id {
    worktree.id
}

/// Whether `path` is inside `root` and not `root` itself (`is_within` with the worktree root).
pub fn is_within(path: &str, root: &str) -> bool {
    let root_path = std::path::Path::new(root);
    let path = std::path::Path::new(path);
    path.starts_with(root_path) && path != root_path
}

/// The result of one removal, as `WorktreeManager.remove` returns it.
pub fn removal_result(worktree_id: &str, removed: bool, reason: Option<&str>) -> Value {
    let mut fields = vec![
        ("worktree_id", text(worktree_id)),
        ("removed", Value::Bool(removed)),
    ];
    if let Some(reason) = reason {
        fields.push(("reason", text(reason)));
    }
    dict(fields)
}

// ------------------------------------------------------------------ removal

/// Decides a removal that went through: the row is removed at the stamp's time and the
/// `worktree.removed` event names its branch, which is kept.
pub fn removed_decision(row: &ManagedWorktree, stamp: &Stamp) -> Decision<()> {
    let (event_id, _) = stamp.ids().next();
    Decision::new(())
        .write(Write::update(ManagedWorktree {
            status: WorktreeStatus::Removed,
            removed_at: Some(stamp.at),
            updated_at: stamp.at,
            ..row.clone()
        }))
        .event(EventDraft {
            attempt_id: row.attempt_id,
            ..EventDraft::new(
                stamp,
                event_id,
                Some(row.workspace_id),
                row.id,
                EventPayload::WorktreeRemoved(WorktreeRemoved {
                    path: row.path.clone(),
                    branch_kept: row.branch.clone(),
                }),
            )
        })
}

/// What `WorktreeManager.remove` answered: whether the worktree was removed, and why not when it
/// was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub worktree_id: Id,
    pub removed: bool,
    pub reason: Option<String>,
}

/// The dict `remove` returns: `worktree_id`, `removed` and `reason` when it was not removed.
pub fn removal_out(removal: &Removal) -> Value {
    let mut fields = vec![
        ("worktree_id", id(removal.worktree_id)),
        ("removed", Value::Bool(removal.removed)),
    ];
    if let Some(reason) = &removal.reason {
        fields.push(("reason", text(reason)));
    }
    dict(fields)
}

impl Removal {
    /// A worktree that was removed.
    pub fn removed(worktree_id: Id) -> Self {
        Removal {
            worktree_id,
            removed: true,
            reason: None,
        }
    }

    /// A worktree that was kept, and why.
    pub fn refused(worktree_id: Id, reason: impl Into<String>) -> Self {
        Removal {
            worktree_id,
            removed: false,
            reason: Some(reason.into()),
        }
    }
}

/// `NotFoundError` for a worktree id.
pub fn not_found(worktree_id: Id) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("managed worktree {worktree_id} not found"),
    )
}

/// A git failure that could not run at all, as a `GitError` of the API.
pub fn git_failure(err: GitError) -> WgError {
    WgError::new(ErrorCode::GitError, err.to_string())
}

/// A file-system failure on `path`.
pub fn io_failure(path: &Path, err: &std::io::Error) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("cannot remove {}: {err}", path.display()),
    )
}
