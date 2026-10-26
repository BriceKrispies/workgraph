//! The pure half of `git worktrees` and `git cleanup-worktrees`: the listing rows and the dry-run
//! plan. Ported from `WorktreeManager.list` and `WorktreeManager.cleanup` in
//! `integrations/git/worktrees.py`; the removal itself is the shell's.

use wg_json::Value;
use wg_model::entity::run::ManagedWorktree;
use wg_types::{Id, WorktreeStatus};
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
