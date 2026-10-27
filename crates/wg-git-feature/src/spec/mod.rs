//! The pure half of the git feature (Pure tier): the decisions and JSON shaping behind `git sync`,
//! `log`, `show`, `link`, `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees` and
//! `cleanup-worktrees`. The service (the crate's `service` module) runs the git processes, loads
//! and writes rows, and calls these functions for every decision and every reply.
//!
//! * [`ingest`]: commit rows from parsed `git log`/`diff-tree` output, branch membership,
//!   reachability updates, and the `SyncResult`.
//! * [`jobs`]: the `git_sync` job's payload, mode and result decisions.
//! * [`links`]: attribution candidates, explicit links and unlinks.
//! * [`reads`]: the JSON of the log, the commit detail, the changes and the diff.
//! * [`analysis`]: the analysis rows of a commit and the change-surface records.
//! * [`worktrees`]: the managed-worktree listing and the cleanup plan.
//! * [`sql`]: the statement text of the git reads and the row decoding (`wg-storecodec`'s
//!   former `git_sql`).
//! * [`cli`]: the clap command of the `git` group and its human views.

pub mod analysis;
pub mod attempt_links;
pub mod cli;
pub mod ingest;
pub mod jobs;
pub mod links;
pub mod reads;
pub mod sql;
pub mod surface;
pub mod worktrees;

use wg_types::{ErrorCode, WgError};

/// Maps a git failure into the error an interface reports (`git_error`).
pub fn git_error(err: wg_gitcore::GitError) -> wg_types::WgError {
    wg_types::WgError::new(wg_types::ErrorCode::GitError, err.to_string())
}

pub fn not_found(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::NotFound, message)
}
