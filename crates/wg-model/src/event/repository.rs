//! Payloads of the `repository.*` events (`core/services/repositories.py`, the git service).

use rpds::VectorSync;
use serde::Serialize;
use wg_types::Id;

use crate::entity::JsonObject;
use crate::event::{Changes, StrMap};

/// `repository.registered` (a new repository) and `repository.checkout_added` (a new checkout of
/// a known one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryAttached {
    pub name: String,
    pub path: String,
    pub kind: String,
    /// `path`, `fingerprint` or `remote` when an existing repository matched; else null.
    pub matched_by: Option<String>,
    pub fingerprint: Option<String>,
    /// Remote name to URL, in git's order.
    pub remotes: StrMap,
}

/// `repository.checkout_discovered`. Two shapes: worktree discovery writes `branch`, `detached`,
/// `locked` and `source`; a refresh that finds a lost checkout again writes `reason`. A key that
/// the shape does not write is `None`; a key written as null is `Some(None)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryCheckoutDiscovered {
    pub path: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Option<String>>,
}

/// `repository.checkout_lost`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryCheckoutLost {
    pub path: String,
    pub kind: String,
    pub reason: Option<String>,
}

/// `repository.checkout_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryCheckoutRemoved {
    pub path: String,
}

/// `repository.changes_detected`: at most 100 sorted ref names of each kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryChangesDetected {
    pub changed_refs: VectorSync<String>,
    pub removed_refs: VectorSync<String>,
    pub non_fast_forward: bool,
}

/// `repository.dependency_declared`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryDependencyDeclared {
    pub depends_on: Id,
    pub depends_on_name: String,
}

/// `repository.health_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryHealthChanged {
    pub from: String,
    pub to: String,
    pub detail: JsonObject,
}

/// `repository.removed`: the registration was archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryRemoved {
    pub name: String,
    pub note: String,
}

/// `repository.updated`: the changed fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepositoryUpdated {
    pub changes: Changes,
}
