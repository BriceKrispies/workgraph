//! Checkouts: which paths a repository is checked out at, the boundary rules that admit a path,
//! the decisions that discover worktrees, and the remote rows a registration writes
//! (`_check_boundary`, `_discover_worktrees`, `_sync_remotes` and `primary_checkout` in
//! `repositories.py`).
//!
//! Paths arrive already resolved (the shell calls `resolve()`); comparisons are component-wise on
//! the text, with `/` and `\` both separators and no case folding.

use serde_json::Value;
use wg_model::RepositoryCheckout;
use wg_types::{ErrorCode, WgError};

use crate::remote::normalize_remote_url;

/// The kind of a discovered checkout: the main working tree is a `clone`, linked trees are
/// `worktree`s.
pub const KIND_CLONE: &str = "clone";
pub const KIND_WORKTREE: &str = "worktree";
/// Kind of the worktrees Workgraph manages itself; never a user checkout.
pub const KIND_MANAGED_WORKTREE: &str = "managed_worktree";

/// The components of a resolved path. A leading separator is kept as a `/` component.
fn components(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    if path.starts_with(['/', '\\']) {
        parts.push("/");
    }
    parts.extend(path.split(['/', '\\']).filter(|part| !part.is_empty()));
    parts
}

/// Whether `child` is `parent` or lies below it (`Path.relative_to` succeeds).
pub fn is_within(child: &str, parent: &str) -> bool {
    components(child).starts_with(&components(parent))
}

/// A path is admitted to a workspace when it lies under one of its allowed roots (when it has
/// any) and not under a Workgraph-managed directory. The messages and details are Python's.
pub fn check_boundary(
    path: &str,
    allowed_roots: &[String],
    managed_roots: &[String],
) -> Result<(), WgError> {
    if !allowed_roots.is_empty() && !allowed_roots.iter().any(|root| is_within(path, root)) {
        let roots = Value::Array(allowed_roots.iter().cloned().map(Value::String).collect());
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("{path} is outside the workspace's allowed roots"),
        )
        .with_detail("path", Value::String(path.to_owned()))
        .with_detail("allowed_roots", roots));
    }
    if managed_roots.iter().any(|root| is_within(path, root)) {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "Workgraph-managed directories cannot be registered as repositories",
        ));
    }
    Ok(())
}

/// One record of `git worktree list`, as the shell parsed it. `path` is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub path: String,
    pub bare: bool,
    pub prunable: bool,
}

/// What to do with one worktree record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// Record a new checkout at `path` of `kind` (`clone` or `worktree`).
    Add { path: String, kind: &'static str },
    /// Leave it out, with the reason the health detail reports.
    Skip { path: String, reason: String },
}

/// Decides, for each worktree record in order, whether it becomes a checkout. Bare trees and
/// paths already known or under a managed root are dropped silently; prunable trees and paths the
/// boundary refuses are skipped with a reason. The record's position decides the kind: the first
/// record (the main working tree) is a `clone`, the rest are `worktree`s. Paths added earlier in
/// the same call count as known for the later records.
pub fn discover_worktrees(
    records: &[WorktreeRecord],
    known: &[String],
    allowed_roots: &[String],
    managed_roots: &[String],
) -> Vec<Discovery> {
    let mut known: Vec<&str> = known.iter().map(String::as_str).collect();
    let mut decisions = Vec::new();
    for (index, record) in records.iter().enumerate() {
        if record.bare {
            continue;
        }
        let path = record.path.as_str();
        if known.contains(&path) || managed_roots.iter().any(|root| is_within(path, root)) {
            continue;
        }
        if record.prunable {
            decisions.push(Discovery::Skip {
                path: path.to_owned(),
                reason: "worktree is prunable".to_owned(),
            });
            continue;
        }
        if let Err(err) = check_boundary(path, allowed_roots, managed_roots) {
            decisions.push(Discovery::Skip {
                path: path.to_owned(),
                reason: err.message,
            });
            continue;
        }
        let kind = if index == 0 {
            KIND_CLONE
        } else {
            KIND_WORKTREE
        };
        known.push(path);
        decisions.push(Discovery::Add {
            path: path.to_owned(),
            kind,
        });
    }
    decisions
}

/// The checkout a repository is reported at: the first user checkout (not managed) that is
/// available, primary ones first, then by creation time. With `require_available` false the first
/// user checkout in that order is returned whether or not it is available.
pub fn primary_checkout(
    checkouts: &[RepositoryCheckout],
    require_available: bool,
) -> Option<&RepositoryCheckout> {
    let mut candidates: Vec<&RepositoryCheckout> = checkouts
        .iter()
        .filter(|checkout| checkout.kind != KIND_MANAGED_WORKTREE)
        .collect();
    candidates.sort_by_key(|checkout| (!checkout.is_primary, checkout.created_at));
    candidates
        .into_iter()
        .find(|checkout| checkout.available || !require_available)
}

/// The remote rows a registration writes: one per remote, with its normalised URL. `update` is
/// true when a remote of that name already exists (its URL is overwritten, not duplicated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWrite {
    pub name: String,
    pub url: String,
    pub normalized_url: String,
    pub update: bool,
}

/// Plans the remote rows for `remotes` (name and URL pairs, in `git remote -v` order) against the
/// names already stored for the repository.
pub fn plan_remote_writes(
    remotes: &[(String, String)],
    existing_names: &[&str],
) -> Vec<RemoteWrite> {
    remotes
        .iter()
        .map(|(name, url)| RemoteWrite {
            name: name.clone(),
            url: url.clone(),
            normalized_url: normalize_remote_url(url),
            update: existing_names.contains(&name.as_str()),
        })
        .collect()
}
