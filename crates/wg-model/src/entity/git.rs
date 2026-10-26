//! The git family: `commits` and the tables whose rows belong to a commit (`commit_file_changes`,
//! `commit_symbols`, `commit_analyses`, `commit_task_links`), the commit equivalences, the object
//! gaps of a repository and the change surface a task leaves in a repository.

use rpds::VectorSync;
use wg_types::{Id, Timestamp};

use crate::entity::{JsonObject, Real};
use crate::write::{Row, RowKey};

/// A row of `commits`: one commit of a repository, as read from git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: Id,
    pub repository_id: Id,
    pub sha: String,
    pub tree_sha: Option<String>,
    /// JSON list of parent SHAs.
    pub parents: VectorSync<String>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: Option<Timestamp>,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_at: Option<Timestamp>,
    pub subject: String,
    pub message: String,
    /// JSON list of trailer objects, in message order.
    pub trailers: VectorSync<JsonObject>,
    pub is_merge: bool,
    pub patch_id: Option<String>,
    pub insertions: i64,
    pub deletions: i64,
    pub files_changed: i64,
    pub reachable: bool,
    pub unreachable_since: Option<Timestamp>,
    pub object_available: bool,
    pub structure_analyzed: bool,
    /// JSON list of branch names that contain the commit.
    pub branches: VectorSync<String>,
    pub ingested_at: Timestamp,
}

/// A row of `commit_file_changes`: one file touched by a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFileChange {
    pub id: Id,
    pub commit_id: Id,
    pub path: String,
    pub old_path: Option<String>,
    /// One of git's letters: `A M D R C T`.
    pub change_type: String,
    pub similarity: Option<i64>,
    pub insertions: i64,
    pub deletions: i64,
    pub is_binary: bool,
    pub language: Option<String>,
    pub file_class: String,
    pub module: String,
}

/// A row of `commit_symbols`: one symbol changed by a commit in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSymbol {
    pub id: Id,
    pub file_change_id: Id,
    pub symbol_kind: String,
    pub name: String,
    /// `added`, `modified` or `deleted`.
    pub change_kind: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub lines_changed: i64,
}

/// A row of `commit_analyses`: the structure analysis of one commit. Keyed by the commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitAnalysis {
    pub commit_id: Id,
    pub analyzer_version: String,
    pub files_parsed: i64,
    pub files_fallback: i64,
    pub functions_affected: i64,
    pub types_affected: i64,
    pub detail: JsonObject,
    pub analyzed_at: Timestamp,
}

/// A row of `commit_equivalences`: two commits that carry the same change (a rewrite or a
/// cherry-pick).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEquivalence {
    pub id: Id,
    pub repository_id: Id,
    pub original_commit_id: Id,
    pub equivalent_commit_id: Id,
    /// `rewrite` or `cherry_pick`.
    pub kind: String,
    pub evidence: JsonObject,
    pub created_at: Timestamp,
}

/// A row of `commit_task_links`: a commit attributed to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTaskLink {
    pub id: Id,
    pub commit_id: Id,
    pub task_id: Id,
    pub source: String,
    pub confidence: Real,
    pub evidence: JsonObject,
    pub attempt_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `git_object_gaps`: a SHA the store saw referenced but whose object is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitObjectGap {
    pub id: Id,
    pub repository_id: Id,
    pub sha: String,
    pub context: String,
    pub detected_at: Timestamp,
}

/// A row of `task_change_surfaces`: what a task changed in one repository, from base to final SHA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskChangeSurface {
    pub id: Id,
    pub task_id: Id,
    pub repository_id: Id,
    pub attempt_id: Option<Id>,
    pub base_sha: String,
    pub final_sha: String,
    pub files_changed: i64,
    pub insertions: i64,
    pub deletions: i64,
    pub renames: i64,
    pub binary_files: i64,
    pub functions_affected: i64,
    pub types_affected: i64,
    pub detail: JsonObject,
    pub computed_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Commit {
    pub fn row_key(&self) -> RowKey {
        RowKey::Commit(self.id)
    }
}

impl CommitFileChange {
    pub fn row_key(&self) -> RowKey {
        RowKey::CommitFileChange(self.id)
    }
}

impl CommitSymbol {
    pub fn row_key(&self) -> RowKey {
        RowKey::CommitSymbol(self.id)
    }
}

impl CommitAnalysis {
    pub fn row_key(&self) -> RowKey {
        RowKey::CommitAnalysis(self.commit_id)
    }
}

impl CommitEquivalence {
    pub fn row_key(&self) -> RowKey {
        RowKey::CommitEquivalence(self.id)
    }
}

impl CommitTaskLink {
    pub fn row_key(&self) -> RowKey {
        RowKey::CommitTaskLink(self.id)
    }
}

impl GitObjectGap {
    pub fn row_key(&self) -> RowKey {
        RowKey::GitObjectGap(self.id)
    }
}

impl TaskChangeSurface {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskChangeSurface(self.id)
    }
}

impl From<Commit> for Row {
    fn from(row: Commit) -> Self {
        Row::Commit(row)
    }
}

impl From<CommitFileChange> for Row {
    fn from(row: CommitFileChange) -> Self {
        Row::CommitFileChange(row)
    }
}

impl From<CommitSymbol> for Row {
    fn from(row: CommitSymbol) -> Self {
        Row::CommitSymbol(row)
    }
}

impl From<CommitAnalysis> for Row {
    fn from(row: CommitAnalysis) -> Self {
        Row::CommitAnalysis(row)
    }
}

impl From<CommitEquivalence> for Row {
    fn from(row: CommitEquivalence) -> Self {
        Row::CommitEquivalence(row)
    }
}

impl From<CommitTaskLink> for Row {
    fn from(row: CommitTaskLink) -> Self {
        Row::CommitTaskLink(row)
    }
}

impl From<GitObjectGap> for Row {
    fn from(row: GitObjectGap) -> Self {
        Row::GitObjectGap(row)
    }
}

impl From<TaskChangeSurface> for Row {
    fn from(row: TaskChangeSurface) -> Self {
        Row::TaskChangeSurface(row)
    }
}
