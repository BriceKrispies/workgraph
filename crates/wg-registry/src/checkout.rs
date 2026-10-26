//! Checkouts: which paths a repository is checked out at, the boundary rules that admit a path,
//! the decisions that discover worktrees, and the remote rows a registration writes
//! (`_check_boundary`, `_discover_worktrees`, `_sync_remotes` and `primary_checkout` in
//! `repositories.py`).
//!
//! Paths arrive already resolved (the shell calls `resolve()`); comparisons are component-wise on
//! the text, with `/` and `\` both separators and no case folding.

use std::iter::once;

use serde_json::{Value, json};
use wg_model::event::RepositoryCheckoutDiscovered;
use wg_model::{EventDraft, EventPayload, RepositoryCheckout, RepositoryRemote, Row, Stamp, Write};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

use crate::remote::normalize_remote_url;

/// The kind of a discovered checkout: the main working tree is a `clone`, linked trees are
/// `worktree`s.
pub const KIND_CLONE: &str = "clone";
pub const KIND_WORKTREE: &str = "worktree";
/// Kind of the worktrees Workgraph manages itself; never a user checkout.
pub const KIND_MANAGED_WORKTREE: &str = "managed_worktree";

/// The components of a resolved path. A leading separator is kept as a `/` component.
fn components(path: &str) -> Vec<&str> {
    path.starts_with(['/', '\\'])
        .then_some("/")
        .into_iter()
        .chain(path.split(['/', '\\']).filter(|part| !part.is_empty()))
        .collect()
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
    let initial: Vec<&str> = known.iter().map(String::as_str).collect();
    records
        .iter()
        .enumerate()
        .fold(
            (initial, Vec::new()),
            |(known, decisions), (index, record)| {
                let Some(decision) =
                    decide_worktree(index, record, &known, allowed_roots, managed_roots)
                else {
                    return (known, decisions);
                };
                let known: Vec<&str> = if matches!(decision, Discovery::Add { .. }) {
                    known
                        .into_iter()
                        .chain(once(record.path.as_str()))
                        .collect()
                } else {
                    known
                };
                (known, decisions.into_iter().chain(once(decision)).collect())
            },
        )
        .1
}

/// The decision for one worktree record, or `None` when it is dropped silently. `known` holds the
/// paths already recorded, including those added earlier in the same call.
fn decide_worktree(
    index: usize,
    record: &WorktreeRecord,
    known: &[&str],
    allowed_roots: &[String],
    managed_roots: &[String],
) -> Option<Discovery> {
    if record.bare {
        return None;
    }
    let path = record.path.as_str();
    if known.contains(&path) || managed_roots.iter().any(|root| is_within(path, root)) {
        return None;
    }
    if record.prunable {
        return Some(Discovery::Skip {
            path: path.to_owned(),
            reason: "worktree is prunable".to_owned(),
        });
    }
    if let Err(err) = check_boundary(path, allowed_roots, managed_roots) {
        return Some(Discovery::Skip {
            path: path.to_owned(),
            reason: err.message,
        });
    }
    let kind = if index == 0 {
        KIND_CLONE
    } else {
        KIND_WORKTREE
    };
    Some(Discovery::Add {
        path: path.to_owned(),
        kind,
    })
}

/// The checkout a repository is reported at: the first user checkout (not managed) that is
/// available, primary ones first, then by creation time. With `require_available` false the first
/// user checkout in that order is returned whether or not it is available.
pub fn primary_checkout(
    checkouts: &[RepositoryCheckout],
    require_available: bool,
) -> Option<&RepositoryCheckout> {
    // The first minimum of the sort key is the first element a stable sort would put first.
    checkouts
        .iter()
        .filter(|checkout| checkout.kind != KIND_MANAGED_WORKTREE)
        .filter(|checkout| checkout.available || !require_available)
        .min_by_key(|checkout| (!checkout.is_primary, checkout.created_at))
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

/// What `git worktree list` says about one record, beyond its path (for the discovery event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDetail {
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
}

/// The `skipped_worktrees` entries of the health detail: the paths a discovery left out.
pub fn skipped_entries(decisions: &[Discovery]) -> Vec<Value> {
    decisions
        .iter()
        .filter_map(|decision| match decision {
            Discovery::Skip { path, reason } => Some(json!({"path": path, "reason": reason})),
            Discovery::Add { .. } => None,
        })
        .collect()
}

/// A new, available checkout row at `path`.
pub fn new_checkout(
    id: Id,
    repository_id: Id,
    path: &str,
    kind: &str,
    is_primary: bool,
    at: Timestamp,
) -> RepositoryCheckout {
    RepositoryCheckout {
        id,
        repository_id,
        path: path.to_owned(),
        kind: kind.to_owned(),
        is_primary,
        access_mode: None,
        available: true,
        last_seen_at: Some(at),
        created_at: at,
        updated_at: at,
    }
}

/// Where a decision's discovered rows take their ids: checkout `i` gets `checkout_slot + i`, its
/// event `event_slot + i`.
#[derive(Debug, Clone, Copy)]
pub struct DiscoverySlots {
    pub checkout: u32,
    pub event: u32,
}

/// The checkouts a discovery adds, each with its `repository.checkout_discovered` event
/// (`source` names the command: `register` or `refresh`).
pub fn discovered_rows(
    decisions: &[Discovery],
    details: &[(WorktreeRecord, WorktreeDetail)],
    stamp: &Stamp,
    scope: (Id, Id),
    source: &str,
    slots: DiscoverySlots,
) -> Vec<(RepositoryCheckout, EventDraft)> {
    let (workspace_id, repository_id) = scope;
    decisions
        .iter()
        .filter_map(|decision| match decision {
            Discovery::Add { path, kind } => Some((path, *kind)),
            Discovery::Skip { .. } => None,
        })
        .enumerate()
        .map(|(index, (path, kind))| {
            let detail = details
                .iter()
                .find(|(record, _)| &record.path == path)
                .map(|(_, detail)| detail);
            let offset = u32::try_from(index).unwrap_or(u32::MAX);
            let checkout = new_checkout(
                stamp.id(slots.checkout + offset),
                repository_id,
                path,
                kind,
                false,
                stamp.at,
            );
            let payload = RepositoryCheckoutDiscovered {
                path: path.clone(),
                kind: kind.to_owned(),
                branch: detail.map(|d| d.branch.clone()),
                detached: detail.map(|d| d.detached),
                locked: detail.map(|d| d.locked),
                source: Some(source.to_owned()),
                reason: None,
            };
            let event = EventDraft::new(
                stamp,
                stamp.id(slots.event + offset),
                Some(workspace_id),
                repository_id,
                EventPayload::RepositoryCheckoutDiscovered(payload),
            );
            (checkout, event)
        })
        .collect()
}

/// The writes that bring a repository's remotes in line with `remotes` (`_sync_remotes`):
/// known names are updated, new ones inserted with ids from `slot` upward.
pub fn remote_writes(
    remotes: &[(String, String)],
    known: &[RepositoryRemote],
    repository_id: Id,
    stamp: &Stamp,
    slot: u32,
) -> Vec<Write> {
    let names: Vec<&str> = known.iter().map(|r| r.name.as_str()).collect();
    plan_remote_writes(remotes, &names)
        .into_iter()
        .enumerate()
        .map(|(index, planned)| {
            let existing = known.iter().find(|r| r.name == planned.name);
            match (planned.update, existing) {
                (true, Some(remote)) => Write::update(Row::RepositoryRemote(RepositoryRemote {
                    url: planned.url.clone(),
                    normalized_url: planned.normalized_url.clone(),
                    updated_at: stamp.at,
                    ..remote.clone()
                })),
                _ => Write::insert(Row::RepositoryRemote(RepositoryRemote {
                    id: stamp.id(slot + u32::try_from(index).unwrap_or(u32::MAX)),
                    repository_id,
                    name: planned.name.clone(),
                    url: planned.url.clone(),
                    normalized_url: planned.normalized_url.clone(),
                    created_at: stamp.at,
                    updated_at: stamp.at,
                })),
            }
        })
        .collect()
}
