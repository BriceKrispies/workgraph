//! `WorktreeManager.capture_outcome` as a decision: the outcome manifest, what the attempt
//! changed, the change set's items, and the `attempt.outcome_captured` event, from what the shell
//! observed in each checkout. The plan itself is `wg_exec::manifest::plan_capture`.

use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_exec::manifest::{CapturePlan, ManifestEntry, NO_COMMITS_DETAIL};
use wg_model::entity::{AgentAttempt, ChangeSet, ChangeSetItem, JsonObject, RevisionManifest};
use wg_model::event::{AttemptOutcomeCaptured, EventDraft, EventPayload};
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_types::{ChangeSetItemStatus, ErrorCode, WgError};

use crate::spec::conclude::Captured;

/// The entries of a baseline manifest (`ManifestEntry.model_validate`).
pub fn read_entries(rows: &[JsonObject]) -> Result<Vec<ManifestEntry>, WgError> {
    rows.iter()
        .map(|row| {
            ManifestEntry::from_json(&Value::Object(row.clone())).ok_or_else(|| {
                WgError::new(
                    ErrorCode::ValidationFailed,
                    "a manifest entry is missing a required field",
                )
            })
        })
        .collect()
}

/// The error when the attempt has no prepared workspace.
pub fn unprepared(attempt: &AgentAttempt) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("attempt {} has no prepared workspace", attempt.id),
    )
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Records the outcome of `attempt`: `plan` from the observations, `commits` the commit range of
/// each entry (aligned with the plan's outcome entries), `change_set` the attempt's change set and
/// its items when it has one.
pub fn capture_decision(
    stamp: &Stamp,
    attempt: &AgentAttempt,
    plan: &CapturePlan,
    commits: &[Vec<String>],
    change_set: Option<&(ChangeSet, Vec<ChangeSetItem>)>,
) -> Decision<Captured> {
    let manifest = RevisionManifest {
        id: stamp.id(0),
        workspace_id: attempt.workspace_id,
        attempt_id: Some(attempt.id),
        kind: "outcome".to_owned(),
        entries: plan
            .outcome
            .iter()
            .map(|entry| object(entry.to_json()))
            .fold(VectorSync::new_sync(), |list, row| list.push_back(row)),
        created_at: stamp.at,
    };
    let violations: Vec<Value> = plan
        .violations
        .iter()
        .map(|found| {
            json!({
                "repository": found.repository,
                "commits": found.commits,
                "uncommitted_files": found.uncommitted_files,
            })
        })
        .collect();
    let dirty: Map<String, Value> = plan
        .dirty
        .iter()
        .map(|(repository, files)| (repository.clone(), json!(files)))
        .collect();
    let telemetry: Map<String, Value> = attempt
        .telemetry
        .iter()
        .filter(|(key, _)| {
            key.as_str() != "uncommitted_changes"
                && (key.as_str() != "read_only_violations" || violations.is_empty())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .chain([("uncommitted_changes".to_owned(), Value::Object(dirty))])
        .chain((!violations.is_empty()).then(|| {
            (
                "read_only_violations".to_owned(),
                Value::Array(violations.clone()),
            )
        }))
        .collect();
    let modified: VectorSync<String> = plan
        .modified_repository_ids
        .iter()
        .cloned()
        .fold(VectorSync::new_sync(), |list, id| list.push_back(id));
    let updated = AgentAttempt {
        outcome_manifest_id: Some(manifest.id),
        modified_repository_ids: modified.clone(),
        telemetry,
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let by_repo: Vec<(String, &Vec<String>)> = plan
        .outcome
        .iter()
        .zip(commits)
        .map(|(entry, shas)| (entry.repository_id.clone(), shas))
        .collect();
    let event = EventDraft::new(
        stamp,
        stamp.id(1),
        Some(attempt.workspace_id),
        attempt.id,
        EventPayload::AttemptOutcomeCaptured(AttemptOutcomeCaptured {
            outcome_manifest_id: manifest.id,
            modified_repositories: modified,
            read_only_violations: violations
                .iter()
                .cloned()
                .map(object)
                .fold(VectorSync::new_sync(), |list, row| list.push_back(row)),
            commits: by_repo
                .iter()
                .map(|(repo, shas)| (repo.clone(), json!(shas.len())))
                .collect(),
        }),
    )
    .with_attempt(Some(attempt.id));
    let base = Decision::new(())
        .write(Write::insert(manifest.clone()))
        .write(Write::update(updated));
    let with_items = match change_set {
        Some((set, items)) => {
            let rows = items.iter().map(|item| {
                let key = item.repository_id.to_string();
                let shas = by_repo
                    .iter()
                    .find(|(repo, _)| *repo == key)
                    .map(|(_, found)| (*found).clone())
                    .unwrap_or_default();
                let (status, detail) = if shas.is_empty() {
                    (
                        ChangeSetItemStatus::Pending,
                        Some(NO_COMMITS_DETAIL.to_owned()),
                    )
                } else {
                    (ChangeSetItemStatus::Committed, None)
                };
                ChangeSetItem {
                    head_sha: plan
                        .outcome
                        .iter()
                        .find(|entry| entry.repository_id == key)
                        .and_then(|entry| entry.sha.clone()),
                    commit_shas: shas
                        .iter()
                        .cloned()
                        .fold(VectorSync::new_sync(), |list, sha| list.push_back(sha)),
                    status,
                    detail,
                    updated_at: stamp.at,
                    ..item.clone()
                }
            });
            let written = rows.fold(base, |decision, row| decision.write(Write::update(row)));
            match plan.change_set_status {
                Some(status) => written.write(Write::update(ChangeSet {
                    status,
                    updated_at: stamp.at,
                    ..set.clone()
                })),
                None => written,
            }
        }
        None => base,
    };
    let report = Captured {
        manifest_id: Some(manifest.id),
        modified: plan.modified_repository_ids.clone(),
        violations: plan
            .violations
            .iter()
            .map(|found| {
                (
                    found.repository.clone(),
                    found.commits,
                    found.uncommitted_files,
                )
            })
            .collect(),
        commits: by_repo
            .iter()
            .map(|(repo, shas)| (repo.clone(), (*shas).clone()))
            .collect(),
        uncommitted: plan.dirty.clone(),
        error: None,
        link_error: None,
    };
    with_items.event(event).with_reply(report)
}
