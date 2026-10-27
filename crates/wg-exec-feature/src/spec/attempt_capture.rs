//! The pure half of `WorktreeManager.capture_outcome`: the records an attempt's outcome leaves
//! once the shell has observed its checkouts (the outcome manifest, the attempt's modified
//! repositories and telemetry, the change set items and the `attempt.outcome_captured` event).

use serde_json::{Map, Value, json};
use wg_exec::manifest::{CapturePlan, ManifestEntry, NO_COMMITS_DETAIL, Violation};
use wg_model::entity::{AgentAttempt, ChangeSet, ChangeSetItem, JsonObject, RevisionManifest};
use wg_model::event::{AttemptOutcomeCaptured, EventDraft, EventPayload};
use wg_model::{Decision, Stamp, Write};
use wg_types::{ChangeSetItemStatus, ChangeSetStatus, Id};

/// What `capture_outcome` reports: the outcome manifest, the repositories that changed, the
/// read-only violations, the commits per repository and the uncommitted files per repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub outcome_manifest_id: Id,
    pub modified: Vec<String>,
    pub violations: Vec<Violation>,
    pub commits: Vec<(String, Vec<String>)>,
    pub uncommitted: Vec<(String, Vec<String>)>,
}

/// The rows a capture updates besides the attempt.
pub struct CaptureRows<'a> {
    pub attempt: &'a AgentAttempt,
    pub change_set: Option<(&'a ChangeSet, &'a [ChangeSetItem])>,
}

/// The violation as the telemetry and the event store it (`repository`, `commits`,
/// `uncommitted_files`).
fn violation_object(violation: &Violation) -> JsonObject {
    json!({
        "repository": violation.repository,
        "commits": violation.commits,
        "uncommitted_files": violation.uncommitted_files,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// `telemetry[key] = value`: an existing key keeps its place, a new one goes last.
fn upsert(map: &Map<String, Value>, key: &str, value: Value) -> Map<String, Value> {
    let present = map.contains_key(key);
    map.iter()
        .map(|(name, member)| {
            if name == key {
                (name.clone(), value.clone())
            } else {
                (name.clone(), member.clone())
            }
        })
        .chain((!present).then(|| (key.to_owned(), value.clone())))
        .collect()
}

fn files_object(files: &[(String, Vec<String>)]) -> Value {
    Value::Object(
        files
            .iter()
            .map(|(name, listed)| (name.clone(), json!(listed)))
            .collect(),
    )
}

/// Decides the records of a capture. `commits` are the commits found per repository id, in the
/// order of the baseline entries.
pub fn capture_decision(
    rows: &CaptureRows<'_>,
    plan: &CapturePlan,
    commits: &[(String, Vec<String>)],
    stamp: &Stamp,
) -> Decision<Captured> {
    let attempt = rows.attempt;
    let (manifest_id, ids) = stamp.ids().next();
    let (event_id, _) = ids.next();
    let manifest = RevisionManifest {
        id: manifest_id,
        workspace_id: attempt.workspace_id,
        attempt_id: Some(attempt.id),
        kind: "outcome".to_owned(),
        entries: plan
            .outcome
            .iter()
            .filter_map(|entry| entry.to_json().as_object().cloned())
            .collect(),
        created_at: stamp.at,
    };
    let dirty = upsert(
        &attempt.telemetry,
        "uncommitted_changes",
        files_object(&plan.dirty),
    );
    let telemetry = if plan.violations.is_empty() {
        dirty
    } else {
        let listed = Value::Array(
            plan.violations
                .iter()
                .map(|violation| Value::Object(violation_object(violation)))
                .collect(),
        );
        upsert(&dirty, "read_only_violations", listed)
    };
    let updated = AgentAttempt {
        outcome_manifest_id: Some(manifest_id),
        modified_repository_ids: plan.modified_repository_ids.iter().cloned().collect(),
        telemetry,
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let item_writes: Vec<Write> = rows
        .change_set
        .map(|(set, items)| {
            let head_by_repo: Vec<(String, Option<String>)> = plan
                .outcome
                .iter()
                .map(|entry: &ManifestEntry| (entry.repository_id.clone(), entry.sha.clone()))
                .collect();
            let updated_items: Vec<ChangeSetItem> = items
                .iter()
                .map(|item| {
                    let key = item.repository_id.to_string();
                    let shas = commits
                        .iter()
                        .find(|(repo, _)| *repo == key)
                        .map(|(_, listed)| listed.clone())
                        .unwrap_or_default();
                    let head = head_by_repo
                        .iter()
                        .find(|(repo, _)| *repo == key)
                        .and_then(|(_, sha)| sha.clone());
                    ChangeSetItem {
                        head_sha: head,
                        status: if shas.is_empty() {
                            ChangeSetItemStatus::Pending
                        } else {
                            ChangeSetItemStatus::Committed
                        },
                        detail: shas.is_empty().then(|| NO_COMMITS_DETAIL.to_owned()),
                        commit_shas: shas.into_iter().collect(),
                        updated_at: stamp.at,
                        ..item.clone()
                    }
                })
                .collect();
            let committed = updated_items
                .iter()
                .filter(|item| item.status == ChangeSetItemStatus::Committed)
                .count();
            let status = if committed == 0 {
                ChangeSetStatus::Open
            } else if committed < updated_items.len() {
                ChangeSetStatus::PartiallyApplied
            } else {
                ChangeSetStatus::Applied
            };
            let set_row = ChangeSet {
                status,
                updated_at: stamp.at,
                ..set.clone()
            };
            updated_items
                .into_iter()
                .map(Write::update)
                .chain(std::iter::once(Write::update(set_row)))
                .collect()
        })
        .unwrap_or_default();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        EventPayload::AttemptOutcomeCaptured(AttemptOutcomeCaptured {
            outcome_manifest_id: manifest_id,
            modified_repositories: plan.modified_repository_ids.iter().cloned().collect(),
            read_only_violations: plan.violations.iter().map(violation_object).collect(),
            commits: commits
                .iter()
                .map(|(repo, listed)| (repo.clone(), json!(listed.len())))
                .collect(),
        }),
    )
    .with_attempt(Some(attempt.id));
    let decision = Decision::new(Captured {
        outcome_manifest_id: manifest_id,
        modified: plan.modified_repository_ids.clone(),
        violations: plan.violations.clone(),
        commits: commits.to_vec(),
        uncommitted: plan.dirty.clone(),
    })
    .write(Write::insert(manifest))
    .write(Write::update(updated));
    item_writes
        .into_iter()
        .fold(decision, Decision::write)
        .event(event)
}

/// Python's `str()` of the violation list (`[{'repository': 'api', 'commits': 1, ...}]`), as the
/// failure text of a read-only violation prints it. Nothing for an empty list.
#[must_use]
pub fn violations_text(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let listed: Vec<String> = violations
        .iter()
        .map(|violation| {
            format!(
                "{{'repository': {}, 'commits': {}, 'uncommitted_files': {}}}",
                wg_model::py_repr_str(&violation.repository),
                violation.commits,
                violation.uncommitted_files
            )
        })
        .collect();
    Some(format!("[{}]", listed.join(", ")))
}

/// The `outcome` of a submit reply: the capture without its commits (`outcome` minus `commits`).
#[must_use]
pub fn outcome_object(captured: &Captured) -> Value {
    json!({
        "outcome_manifest_id": captured.outcome_manifest_id.to_string(),
        "modified": captured.modified,
        "violations": captured.violations.iter().map(violation_object).collect::<Vec<_>>(),
        "uncommitted": files_object(&captured.uncommitted),
    })
}
