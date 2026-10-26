//! The pure half of the `git` command group (IF-12): the decisions and JSON shaping behind `git
//! sync`, `log`, `show`, `link`, `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees`
//! and `cleanup-worktrees`. The shell (`wg-runtime`'s `git` module) runs the git processes, loads
//! and writes rows, and calls these functions for every decision and every reply.
//!
//! * [`ingest`]: commit rows from parsed `git log`/`diff-tree` output, branch membership,
//!   reachability updates, and the `SyncResult`.
//! * [`links`]: attribution candidates, explicit links and unlinks.
//! * [`reads`]: the JSON of the log, the commit detail, the changes and the diff.
//! * [`analysis`]: the analysis rows of a commit and the change-surface records.
//! * [`worktrees`]: the managed-worktree listing and the cleanup plan.

pub mod analysis;
pub mod ingest;
pub mod links;
pub mod reads;
pub mod worktrees;

/// Maps a git failure into the error an interface reports (`git_error`).
pub fn git_error(err: wg_gitcore::GitError) -> wg_types::WgError {
    wg_types::WgError::new(wg_types::ErrorCode::GitError, err.to_string())
}
