//! Payloads of the `git.*` events (`integrations/git/service.py`).

use rpds::VectorSync;
use serde::Serialize;
use serde_json::Value;
use wg_types::Id;

/// `git.commit_ingested`. Written against the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitIngested {
    pub deletions: i64,
    pub insertions: i64,
    pub repository_id: Id,
    pub sha: String,
    pub subject: String,
    pub task_ids: VectorSync<Id>,
}

/// `git.commits_attributed`. `repository_id` is present only for the repository-scoped form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitsAttributed {
    pub links_created: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<Id>,
}

/// `git.sync_completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitSyncCompleted {
    pub equivalences: Value,
    pub head_refs: Value,
    pub links_created: i64,
    pub marked_unreachable: i64,
    pub mode: String,
    pub new_commits: i64,
    pub refs_changed: Value,
    pub repository: String,
    pub repository_id: Id,
    pub restored_reachable: i64,
    pub shallow: bool,
}
