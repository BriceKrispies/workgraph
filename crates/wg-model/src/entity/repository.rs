//! The `repository` family: `repositories`, `repository_checkouts`, `repository_remotes`,
//! `repository_refs` and `repository_dependencies`.

use rpds::VectorSync;
use wg_types::{AccessMode, Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `repositories`: the canonical repository identity, independent of path or remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: Id,
    pub workspace_id: Id,
    pub name: String,
    pub description: String,
    /// Sorted root-commit SHAs; recognises the same history at a new path or remote.
    pub identity_fingerprint: Option<String>,
    pub default_branch: Option<String>,
    /// JSON list column.
    pub tracked_branches: VectorSync<String>,
    pub access_mode: AccessMode,
    /// `unknown`, `ok`, `degraded` or `unavailable`.
    pub health_status: String,
    pub health_detail: JsonObject,
    pub health_checked_at: Option<Timestamp>,
    pub is_shallow: bool,
    pub last_scanned_at: Option<Timestamp>,
    pub archived_at: Option<Timestamp>,
    /// The `metadata` column (`metadata_` in the Python ORM).
    pub metadata: JsonObject,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `repository_checkouts`: one path where the repository is checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCheckout {
    pub id: Id,
    pub repository_id: Id,
    pub path: String,
    /// `clone`, `worktree`, `managed_worktree`, ...
    pub kind: String,
    pub is_primary: bool,
    pub access_mode: Option<String>,
    pub available: bool,
    pub last_seen_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `repository_remotes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRemote {
    pub id: Id,
    pub repository_id: Id,
    pub name: String,
    pub url: String,
    pub normalized_url: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `repository_refs`: the last observed tip of one ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub id: Id,
    pub repository_id: Id,
    pub name: String,
    pub sha: String,
    pub updated_at: Timestamp,
}

/// A row of `repository_dependencies`: a declared (not observed) dependency between repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryDependency {
    pub id: Id,
    pub from_repository_id: Id,
    pub to_repository_id: Id,
    /// `depends_on` unless the caller named another kind.
    pub kind: String,
    pub description: String,
    pub created_at: Timestamp,
}

impl Repository {
    pub fn row_key(&self) -> RowKey {
        RowKey::Repository(self.id)
    }
}

impl RepositoryCheckout {
    pub fn row_key(&self) -> RowKey {
        RowKey::RepositoryCheckout(self.id)
    }
}

impl RepositoryRemote {
    pub fn row_key(&self) -> RowKey {
        RowKey::RepositoryRemote(self.id)
    }
}

impl RepositoryRef {
    pub fn row_key(&self) -> RowKey {
        RowKey::RepositoryRef(self.id)
    }
}

impl RepositoryDependency {
    pub fn row_key(&self) -> RowKey {
        RowKey::RepositoryDependency(self.id)
    }
}

impl From<Repository> for Row {
    fn from(row: Repository) -> Self {
        Row::Repository(row)
    }
}

impl From<RepositoryCheckout> for Row {
    fn from(row: RepositoryCheckout) -> Self {
        Row::RepositoryCheckout(row)
    }
}

impl From<RepositoryRemote> for Row {
    fn from(row: RepositoryRemote) -> Self {
        Row::RepositoryRemote(row)
    }
}

impl From<RepositoryRef> for Row {
    fn from(row: RepositoryRef) -> Self {
        Row::RepositoryRef(row)
    }
}

impl From<RepositoryDependency> for Row {
    fn from(row: RepositoryDependency) -> Self {
        Row::RepositoryDependency(row)
    }
}
