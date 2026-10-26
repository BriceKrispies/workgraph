//! Registering a repository (`RepositoryService.register` in `repositories.py`) as one decision.
//!
//! The shell runs Git, reads the lookups a registration can match and loads the queued sync job
//! of the repository it may match. This module decides which repository the registration joins,
//! and returns the writes, events and `git_sync` job that record it. Its reply is the repository
//! id. Ids are derived from the stamp by fixed slots per kind of row, so the decision is a pure
//! function of its inputs.

use std::iter::once;

use rpds::{RedBlackTreeMapSync, VectorSync};
use serde_json::{Map, Value, json};
use wg_jobs::queue::{NewJob, enqueue};
use wg_model::entity::{Job, Repository, RepositoryCheckout, RepositoryRemote};
use wg_model::event::{RepositoryAttached, RepositoryCheckoutDiscovered};
use wg_model::slice::QueueSlice;
use wg_model::{Decision, EventDraft, EventPayload, Row, Stamp, StrMap, Write};
use wg_types::{AccessMode, Id, Permission, WgError};

use crate::checkout::{
    Discovery, KIND_MANAGED_WORKTREE, WorktreeRecord, check_boundary, discover_worktrees,
    plan_remote_writes,
};
use crate::identity::{MatchedBy, matched_by, new_repository_name};

/// Id slots: the repository, then one range per kind of row, offset by the row's position.
const REPOSITORY_SLOT: u32 = 1;
const CHECKOUT_SLOT: u32 = 0x100;
const REMOTE_SLOT: u32 = 0x1_0000;
const DISCOVERED_EVENT_SLOT: u32 = 0x2_0000;
const REGISTERED_EVENT_SLOT: u32 = 0x3_0000;

/// What `git worktree list` says about one record, beyond its path (for the discovery event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDetail {
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
}

/// The registration request, as the shell read it from the path (Git facts already gathered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub workspace_id: Id,
    /// The resolved top of the working tree (`git rev-parse --show-toplevel`).
    pub path: String,
    /// The last component of `path`, the name a repository gets when none is requested.
    pub dir_name: String,
    pub requested_name: Option<String>,
    pub read_only: bool,
    /// `identity::fingerprint` of the root commits, `None` for an empty history.
    pub fingerprint: Option<String>,
    /// Remote name and URL pairs, in `git remote -v` order.
    pub remotes: Vec<(String, String)>,
    /// `clone` for the main working tree, `worktree` for a linked one.
    pub kind: &'static str,
    pub default_branch: Option<String>,
    pub shallow: bool,
    /// Every worktree record of `git worktree list`, in git's order, with its detail.
    pub worktrees: Vec<(WorktreeRecord, WorktreeDetail)>,
    pub allowed_roots: Vec<String>,
    pub managed_roots: Vec<String>,
}

/// A repository a lookup found, with the rows the decision reads from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub repository: Repository,
    pub checkouts: Vec<RepositoryCheckout>,
    pub remotes: Vec<RepositoryRemote>,
}

/// What the store held when the decision was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The repository with a checkout at the request's path, if any.
    pub by_path: Option<Matched>,
    /// The repository with the request's fingerprint, if any.
    pub by_fingerprint: Option<Matched>,
    /// The repository with a remote whose normalised URL the request has, if any.
    pub by_remote: Option<Matched>,
    /// The names of every repository in the workspace, archived ones included.
    pub taken_names: Vec<String>,
    /// The queued `git_sync` job of the matched repository, when one exists (see [`sync_key`]).
    pub sync_job: Option<Job>,
}

/// The idempotency key of the `git_sync` job a registration queues for `repository_id`.
pub fn sync_key(repository_id: Id, path: &str) -> String {
    format!("git_sync:{repository_id}:register:{path}")
}

/// The match the request joins, in the order `identity::matched_by` gives: a checkout at the
/// path, then the fingerprint, then the remote.
fn chosen(facts: &Facts) -> Option<(&Matched, MatchedBy)> {
    let by_id = |found: &Option<Matched>| found.as_ref().map(|m| m.repository.id);
    let (id, by) = matched_by(
        by_id(&facts.by_path),
        by_id(&facts.by_fingerprint),
        by_id(&facts.by_remote),
    )?;
    [&facts.by_path, &facts.by_fingerprint, &facts.by_remote]
        .into_iter()
        .flatten()
        .find(|m| m.repository.id == id)
        .map(|m| (m, by))
}

/// The merged health detail: the old keys with `skipped_worktrees` set to `skipped`.
fn health_detail(old: &Map<String, Value>, skipped: Vec<Value>) -> Map<String, Value> {
    const KEY: &str = "skipped_worktrees";
    let list = Value::Array(skipped);
    let replaced = old.iter().map(|(key, value)| {
        let value = if key == KEY {
            list.clone()
        } else {
            value.clone()
        };
        (key.clone(), value)
    });
    let added = (!old.contains_key(KEY)).then(|| (KEY.to_owned(), list.clone()));
    replaced.chain(added).collect()
}

/// A new checkout row at `path`.
fn new_checkout(
    id: Id,
    repository_id: Id,
    path: &str,
    kind: &str,
    is_primary: bool,
    at: wg_types::Timestamp,
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

/// The decision of a registration: writes, events and the `git_sync` job it queues.
///
/// Errors: `permission_denied` without `repo:register`; a `validation_failed` for a path the
/// workspace's boundary refuses; `conflict` for an explicit name already taken.
pub fn register(request: &Request, facts: &Facts, stamp: &Stamp) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::RepoRegister])?;
    check_boundary(
        &request.path,
        &request.allowed_roots,
        &request.managed_roots,
    )?;
    let at = stamp.at;
    let ws = request.workspace_id;

    // The repository the request joins, or the one it creates.
    let (base, created, matched, known) = match chosen(facts) {
        Some((found, by)) => (
            found.repository.clone(),
            false,
            Some(by),
            found.checkouts.clone(),
        ),
        None => {
            let taken: Vec<&str> = facts.taken_names.iter().map(String::as_str).collect();
            let name =
                new_repository_name(request.requested_name.as_deref(), &request.dir_name, &taken)?;
            let repository = Repository {
                id: stamp.id(REPOSITORY_SLOT),
                workspace_id: ws,
                name,
                description: String::new(),
                identity_fingerprint: request.fingerprint.clone(),
                default_branch: request.default_branch.clone(),
                tracked_branches: request.default_branch.iter().cloned().collect(),
                access_mode: if request.read_only {
                    AccessMode::ReadOnly
                } else {
                    AccessMode::ReadWrite
                },
                health_status: "ok".to_owned(),
                health_detail: Map::new(),
                health_checked_at: Some(at),
                is_shallow: request.shallow,
                last_scanned_at: None,
                archived_at: None,
                metadata: Map::new(),
                created_at: at,
                updated_at: at,
            };
            (repository, true, None, Vec::new())
        }
    };
    let repo_id = base.id;
    let known_remotes: Vec<RepositoryRemote> = chosen(facts)
        .map(|(found, _)| found.remotes.clone())
        .unwrap_or_default();

    // The checkout at the request's path: refreshed when it exists, added otherwise.
    let existing = known.iter().find(|c| c.path == request.path).cloned();
    let (main_checkout, main_write) = match existing {
        Some(checkout) => {
            let refreshed = RepositoryCheckout {
                available: true,
                last_seen_at: Some(at),
                updated_at: at,
                ..checkout
            };
            let write = Write::update(Row::RepositoryCheckout(refreshed.clone()));
            (refreshed, write)
        }
        None => {
            let is_primary = !known.iter().any(|c| c.kind != KIND_MANAGED_WORKTREE);
            let checkout = new_checkout(
                stamp.id(CHECKOUT_SLOT),
                repo_id,
                &request.path,
                request.kind,
                is_primary,
                at,
            );
            let write = Write::insert(Row::RepositoryCheckout(checkout.clone()));
            (checkout, write)
        }
    };
    let checkouts: Vec<RepositoryCheckout> = known
        .iter()
        .filter(|c| c.path != request.path)
        .cloned()
        .chain(once(main_checkout))
        .collect();

    // Linked worktrees that are not checkouts yet, in git's order (`_discover_worktrees`).
    let known_paths: Vec<String> = checkouts.iter().map(|c| c.path.clone()).collect();
    let records: Vec<WorktreeRecord> = request.worktrees.iter().map(|(r, _)| r.clone()).collect();
    let decisions = discover_worktrees(
        &records,
        &known_paths,
        &request.allowed_roots,
        &request.managed_roots,
    );
    let skipped: Vec<Value> = decisions
        .iter()
        .filter_map(|decision| match decision {
            Discovery::Skip { path, reason } => Some(json!({"path": path, "reason": reason})),
            Discovery::Add { .. } => None,
        })
        .collect();
    let discovered: Vec<(RepositoryCheckout, EventDraft)> = decisions
        .iter()
        .filter_map(|decision| match decision {
            Discovery::Add { path, kind } => Some((path, *kind)),
            Discovery::Skip { .. } => None,
        })
        .enumerate()
        .map(|(index, (path, kind))| {
            let detail = request
                .worktrees
                .iter()
                .find(|(record, _)| &record.path == path)
                .map(|(_, detail)| detail);
            let checkout = new_checkout(
                stamp.id(CHECKOUT_SLOT + 1 + index as u32),
                repo_id,
                path,
                kind,
                false,
                at,
            );
            let payload = RepositoryCheckoutDiscovered {
                path: path.clone(),
                kind: kind.to_owned(),
                branch: detail.map(|d| d.branch.clone()),
                detached: detail.map(|d| d.detached),
                locked: detail.map(|d| d.locked),
                source: Some("register".to_owned()),
                reason: None,
            };
            let event = EventDraft::new(
                stamp,
                stamp.id(DISCOVERED_EVENT_SLOT + index as u32),
                Some(ws),
                repo_id,
                EventPayload::RepositoryCheckoutDiscovered(payload),
            );
            (checkout, event)
        })
        .collect();

    // The remotes: existing ones by name are updated, the others added.
    let names: Vec<&str> = known_remotes.iter().map(|r| r.name.as_str()).collect();
    let remote_writes: Vec<Write> = plan_remote_writes(&request.remotes, &names)
        .into_iter()
        .enumerate()
        .map(|(index, planned)| {
            let existing_remote = known_remotes.iter().find(|r| r.name == planned.name);
            match (planned.update, existing_remote) {
                (true, Some(remote)) => Write::update(Row::RepositoryRemote(RepositoryRemote {
                    url: planned.url.clone(),
                    normalized_url: planned.normalized_url.clone(),
                    updated_at: at,
                    ..remote.clone()
                })),
                _ => Write::insert(Row::RepositoryRemote(RepositoryRemote {
                    id: stamp.id(REMOTE_SLOT + index as u32),
                    repository_id: repo_id,
                    name: planned.name.clone(),
                    url: planned.url.clone(),
                    normalized_url: planned.normalized_url.clone(),
                    created_at: at,
                    updated_at: at,
                })),
            }
        })
        .collect();

    // The repository row, with the health detail that discovery's skips produced.
    let repository = Repository {
        archived_at: None,
        identity_fingerprint: base
            .identity_fingerprint
            .clone()
            .or_else(|| request.fingerprint.clone()),
        health_detail: health_detail(&base.health_detail, skipped),
        updated_at: at,
        ..base
    };
    let repository_write = if created {
        Write::insert(Row::Repository(repository.clone()))
    } else {
        Write::update(Row::Repository(repository.clone()))
    };

    // The registration event, after the discovery events.
    let attached = RepositoryAttached {
        name: repository.name.clone(),
        path: request.path.clone(),
        kind: request.kind.to_owned(),
        matched_by: matched.map(|by| by.as_str().to_owned()),
        fingerprint: request.fingerprint.clone(),
        remotes: StrMap(request.remotes.iter().cloned().collect()),
    };
    let payload = if created {
        EventPayload::RepositoryRegistered(attached)
    } else {
        EventPayload::RepositoryCheckoutAdded(attached)
    };
    let registered = EventDraft::new(
        stamp,
        stamp.id(REGISTERED_EVENT_SLOT),
        Some(ws),
        repo_id,
        payload,
    );

    // The `git_sync` job, idempotent on its key: an existing job makes the enqueue a no-op.
    let job = NewJob {
        payload: json!({"repository_id": repo_id.to_string(), "mode": "incremental"})
            .as_object()
            .cloned()
            .unwrap_or_default(),
        workspace_id: Some(ws),
        idempotency_key: Some(sync_key(repo_id, &request.path)),
        resource_keys: vec![format!("repo:{repo_id}")],
        correlation_id: Some(stamp.correlation),
        causation_id: stamp.causation,
        created_by: stamp.actor.to_string(),
        ..NewJob::new("git_sync")
    };
    let queue = QueueSlice {
        jobs: match facts.sync_job.clone() {
            Some(existing) => RedBlackTreeMapSync::new_sync().insert(existing.id, existing),
            None => RedBlackTreeMapSync::new_sync(),
        },
        ..QueueSlice::default()
    };
    let queued = enqueue(&queue, &job, stamp);

    let writes: Vec<Write> = once(repository_write)
        .chain(once(main_write))
        .chain(
            discovered
                .iter()
                .map(|(checkout, _)| Write::insert(Row::RepositoryCheckout(checkout.clone()))),
        )
        .chain(remote_writes)
        .chain(queued.writes.iter().cloned())
        .collect();
    let events: Vec<EventDraft> = discovered
        .into_iter()
        .map(|(_, event)| event)
        .chain(once(registered))
        .chain(queued.events.iter().cloned())
        .collect();
    let effects: Vec<_> = queued.effects.iter().cloned().collect();

    Ok(Decision {
        writes: VectorSync::from_iter(writes),
        events: VectorSync::from_iter(events),
        effects: VectorSync::from_iter(effects),
        reply: repo_id,
    })
}
