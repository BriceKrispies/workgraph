//! Refreshing a repository (`RepositoryService.refresh` in `repositories.py`): verify every
//! checkout, rediscover worktrees, refresh the remotes, default branch and shallow state, and
//! record the health.
//!
//! The shell probes each checkout with git and passes the facts in as [`Probe`]s, parallel to the
//! repository's checkouts. [`observations`] turns them into what each checkout is; the shell uses
//! [`first_live`] to pick the checkout whose worktree list it reads, and [`refresh`] decides.

use std::iter::once;

use serde_json::{Map, Value, json};
use wg_model::event::{
    RepositoryCheckoutDiscovered, RepositoryCheckoutLost, RepositoryHealthChanged,
};
use wg_model::{
    Decision, EventDraft, EventPayload, Repository, RepositoryCheckout, Row, Stamp, Write,
};
use wg_types::{Permission, WgError};

use crate::checkout::{
    DiscoverySlots, WorktreeDetail, WorktreeRecord, discover_worktrees, discovered_rows,
    remote_writes, skipped_entries,
};
use crate::health::{Observation, health_status, observe, with_prunable};
use crate::register::Matched;

const CHECKOUT_SLOT: u32 = 0x100;
const CHANGE_EVENT_SLOT: u32 = 0x1000;
const REMOTE_SLOT: u32 = 0x1_0000;
const DISCOVERED_EVENT_SLOT: u32 = 0x2_0000;
const HEALTH_EVENT_SLOT: u32 = 0x3_0000;

/// What git said about one checkout path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub is_repository: bool,
    /// `identity::fingerprint` of the root commits; `None` for an empty history.
    pub fingerprint: Option<String>,
    pub remotes: Vec<(String, String)>,
    pub default_branch: Option<String>,
    pub shallow: bool,
}

/// The refresh request: one probe per checkout of the repository, in the order of
/// `Matched::checkouts`, and the worktree list of the first live checkout (empty when none is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub probes: Vec<Probe>,
    pub worktrees: Vec<(WorktreeRecord, WorktreeDetail)>,
    pub allowed_roots: Vec<String>,
    pub managed_roots: Vec<String>,
}

/// What each checkout is, before the prunable rule: a missing repository, other history or live.
pub fn observations(matched: &Matched, probes: &[Probe]) -> Vec<Observation> {
    probes
        .iter()
        .map(|probe| {
            observe(
                probe.is_repository,
                matched.repository.identity_fingerprint.as_deref(),
                probe.fingerprint.as_deref(),
            )
        })
        .collect()
}

/// The index of the first live checkout, whose worktree list the refresh reads.
pub fn first_live(observed: &[Observation]) -> Option<usize> {
    observed.iter().position(|o| o.available())
}

/// The remotes, default branch and shallow state: those of the first live checkout whose remotes
/// are not empty, or of the last live one when all are (Python overwrites while `remotes` is empty).
fn repository_facts(
    observed: &[Observation],
    probes: &[Probe],
) -> (Vec<(String, String)>, Option<String>, bool) {
    observed
        .iter()
        .zip(probes)
        .filter(|(o, _)| o.available())
        .fold((Vec::new(), None, false), |found, (_, probe)| {
            if found.0.is_empty() {
                (
                    probe.remotes.clone(),
                    probe.default_branch.clone(),
                    probe.shallow,
                )
            } else {
                found
            }
        })
}

/// The health detail entry of one checkout.
fn detail_entry(path: &str, observation: Observation) -> Value {
    let base = Map::from_iter([
        ("path".to_owned(), json!(path)),
        ("available".to_owned(), json!(observation.available())),
    ]);
    Value::Object(
        base.into_iter()
            .chain(observation.error().map(|e| ("error".to_owned(), json!(e))))
            .collect(),
    )
}

/// The decision of a refresh. Errors: `permission_denied` without `repo:read`.
pub fn refresh(
    request: &Request,
    matched: &Matched,
    stamp: &Stamp,
) -> Result<Decision<Repository>, WgError> {
    stamp.require(&[Permission::RepoRead])?;
    let at = stamp.at;
    let repo = &matched.repository;
    let ws = repo.workspace_id;
    let observed = observations(matched, &request.probes);
    let (remotes, default_branch, shallow) = repository_facts(&observed, &request.probes);

    // A prunable worktree is not available, whatever the probe said.
    let prunable: Vec<&str> = request
        .worktrees
        .iter()
        .filter(|(record, _)| record.prunable)
        .map(|(record, _)| record.path.as_str())
        .collect();
    let finals: Vec<Observation> = matched
        .checkouts
        .iter()
        .zip(&observed)
        .map(|(checkout, observation)| {
            with_prunable(
                *observation,
                &checkout.kind,
                prunable.contains(&checkout.path.as_str()),
            )
        })
        .collect();

    // The checkouts, refreshed; a worktree that appears or disappears is an event.
    let refreshed: Vec<(RepositoryCheckout, Option<EventPayload>)> = matched
        .checkouts
        .iter()
        .zip(&finals)
        .map(|(checkout, observation)| {
            let available = observation.available();
            let change = (checkout.kind == crate::checkout::KIND_WORKTREE
                && available != checkout.available)
                .then(|| change_payload(checkout, *observation));
            let row = RepositoryCheckout {
                available,
                last_seen_at: if available {
                    Some(at)
                } else {
                    checkout.last_seen_at
                },
                updated_at: if available || available != checkout.available {
                    at
                } else {
                    checkout.updated_at
                },
                ..checkout.clone()
            };
            (row, change)
        })
        .collect();
    let change_events: Vec<EventDraft> = refreshed
        .iter()
        .filter_map(|(_, payload)| payload.clone())
        .enumerate()
        .map(|(index, payload)| {
            EventDraft::new(
                stamp,
                stamp.id(CHANGE_EVENT_SLOT + u32::try_from(index).unwrap_or(u32::MAX)),
                Some(ws),
                repo.id,
                payload,
            )
        })
        .collect();

    // Linked worktrees that are not checkouts yet.
    let known: Vec<String> = matched.checkouts.iter().map(|c| c.path.clone()).collect();
    let records: Vec<WorktreeRecord> = request.worktrees.iter().map(|(r, _)| r.clone()).collect();
    let decisions = discover_worktrees(
        &records,
        &known,
        &request.allowed_roots,
        &request.managed_roots,
    );
    let skipped = skipped_entries(&decisions);
    let discovered = discovered_rows(
        &decisions,
        &request.worktrees,
        stamp,
        (ws, repo.id),
        "refresh",
        DiscoverySlots {
            checkout: CHECKOUT_SLOT,
            event: DISCOVERED_EVENT_SLOT,
        },
    );

    // The repository row and its health.
    let all_checkouts: Vec<RepositoryCheckout> = refreshed
        .iter()
        .map(|(row, _)| row.clone())
        .chain(discovered.iter().map(|(row, _)| row.clone()))
        .collect();
    let status = health_status(&all_checkouts);
    let detail: Map<String, Value> = Map::from_iter([
        (
            "checkouts".to_owned(),
            Value::Array(
                matched
                    .checkouts
                    .iter()
                    .zip(&finals)
                    .map(|(checkout, observation)| detail_entry(&checkout.path, *observation))
                    .collect(),
            ),
        ),
        ("skipped_worktrees".to_owned(), Value::Array(skipped)),
    ]);
    let row = Repository {
        default_branch: match default_branch.filter(|b| !b.is_empty()) {
            Some(found) if repo.default_branch.as_deref().is_none_or(str::is_empty) => Some(found),
            _ => repo.default_branch.clone(),
        },
        is_shallow: shallow,
        health_status: status.to_owned(),
        health_detail: detail.clone(),
        health_checked_at: Some(at),
        updated_at: at,
        ..repo.clone()
    };
    // The reply is the row as written, so `health_detail` keeps the key order it was built with;
    // a reload from the store sorts JSON object keys.
    let reply = row.clone();
    let health_event = (repo.health_status != status).then(|| {
        let payload = RepositoryHealthChanged {
            from: repo.health_status.clone(),
            to: status.to_owned(),
            detail,
        };
        EventDraft::new(
            stamp,
            stamp.id(HEALTH_EVENT_SLOT),
            Some(ws),
            repo.id,
            EventPayload::RepositoryHealthChanged(payload),
        )
    });

    let remote_changes = if remotes.is_empty() {
        Vec::new()
    } else {
        remote_writes(&remotes, &matched.remotes, repo.id, stamp, REMOTE_SLOT)
    };
    let writes: Vec<Write> = refreshed
        .iter()
        .zip(&matched.checkouts)
        .filter(|((row, _), old)| row != *old)
        .map(|((row, _), _)| Write::update(Row::RepositoryCheckout(row.clone())))
        .chain(
            discovered
                .iter()
                .map(|(row, _)| Write::insert(Row::RepositoryCheckout(row.clone()))),
        )
        .chain(remote_changes)
        .chain(once(Write::update(Row::Repository(row))))
        .collect();
    let events: Vec<EventDraft> = change_events
        .into_iter()
        .chain(discovered.into_iter().map(|(_, event)| event))
        .chain(health_event)
        .collect();
    let decision = writes
        .into_iter()
        .fold(Decision::new(reply), Decision::write);
    Ok(events.into_iter().fold(decision, Decision::event))
}

/// The event of a worktree whose availability changed: found again or lost.
fn change_payload(checkout: &RepositoryCheckout, observation: Observation) -> EventPayload {
    let reason = observation.error().map(str::to_owned);
    if observation.available() {
        EventPayload::RepositoryCheckoutDiscovered(RepositoryCheckoutDiscovered {
            path: checkout.path.clone(),
            kind: checkout.kind.clone(),
            branch: None,
            detached: None,
            locked: None,
            source: None,
            reason: Some(reason),
        })
    } else {
        EventPayload::RepositoryCheckoutLost(RepositoryCheckoutLost {
            path: checkout.path.clone(),
            kind: checkout.kind.clone(),
            reason,
        })
    }
}
