//! Payloads of the `worktree.*` events (`integrations/git/worktrees.py` and the stack
//! integration and verification services that create managed worktrees).

use serde::Serialize;
use wg_types::Id;

/// `worktree.created`: `purpose`, `stack_id`, `base` and `branch` are present for some callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeCreated {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_id: Option<Id>,
    pub repository: String,
    pub path: String,
    pub base_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// `worktree.removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeRemoved {
    pub path: String,
    pub branch_kept: bool,
}

/// `worktree.recovered`: how many worktree directories recovery touched, by outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeRecovered {
    pub activated: i64,
    pub orphaned: i64,
    pub released: i64,
    pub unknown_dirs: i64,
}
