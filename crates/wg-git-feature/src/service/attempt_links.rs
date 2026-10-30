//! `GitService.link_attempt_commits`: the commits an attempt made between its baseline and its
//! outcome manifest are ingested when the store lacks them and linked to the attempt's task as
//! execution context.

use std::collections::BTreeSet;
use std::path::PathBuf;

use wg_git::Git;
use wg_model::entity::Real;
use wg_model::entity::git::CommitTaskLink;
use wg_model::event::{EntityType, EventDraft, EventPayload, GitCommitsAttributed};
use wg_model::{Decision, Row, Write};
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_types::{Id, WgError};

use super::entries::analyze_commit;
use super::store;
use super::sync::{detect_equivalences, ingest_batch, repo_state};
use crate::spec::attempt_links;
use crate::spec::git_error;
use crate::spec::ingest::offset_time;

/// The source name of an execution-context link (`AttributionSource.EXECUTION_CONTEXT`).
const EXECUTION_CONTEXT: &str = "execution_context";
/// `ATTRIBUTION_CONFIDENCE[EXECUTION_CONTEXT]`.
const CONFIDENCE: f64 = 0.95;
/// The actor of these links.
const ACTOR: &str = "system:runner";
/// The timeout of the git calls, in milliseconds.
const TIMEOUT_MS: u64 = 600_000;

/// Links the commits between the baseline and outcome manifests of `attempt_id` to its task.
/// Returns the number of links created; an attempt without both manifests links nothing.
pub fn link_attempt_commits(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
) -> Result<i64, WgError> {
    let loaded = app.runtime.store.read(|session| {
        let attempt = session
            .attempt_by_id(attempt_id)?
            .ok_or_else(|| WgError::not_found(format!("attempt '{attempt_id}' not found")))?;
        let manifests = session.manifests_of_attempt(attempt_id)?;
        let pick = |id: Option<Id>| {
            id.and_then(|wanted| manifests.iter().find(|row| row.id == wanted).cloned())
        };
        Ok((
            pick(attempt.baseline_manifest_id),
            pick(attempt.outcome_manifest_id),
            attempt.task_id,
            attempt.workspace_id,
        ))
    })?;
    let (Some(baseline), Some(outcome), task_id, workspace_id) = loaded else {
        return Ok(0);
    };
    let ranges = attempt_links::ranges(&baseline.entries, &outcome.entries);
    let mut created: i64 = 0;
    for range in ranges {
        let git = Git {
            path: PathBuf::from(&range.path),
            timeout_ms: TIMEOUT_MS,
        };
        let shas = git
            .rev_list_range(&range.base, &range.head)
            .map_err(git_error)?;
        let state = repo_state(app, range.repository_id)?;
        let known: BTreeSet<String> = app.runtime.store.read(|session| {
            Ok(store::commit_shas(session, range.repository_id)?
                .into_iter()
                .collect())
        })?;
        let missing: Vec<String> = shas
            .iter()
            .filter(|sha| !known.contains(*sha))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let made = ingest_batch(app, context, &git, &state, &missing)?;
            let new_ids: Vec<Id> = made.iter().map(|(id, _)| *id).collect();
            detect_equivalences(app, context, &state, &new_ids)?;
            for id in &new_ids {
                analyze_commit(app, context, *id)?;
            }
        }
        let commits = app.runtime.store.read(|session| {
            let mut found = Vec::new();
            for sha in &shas {
                if let Some(commit) = store::commit_by_sha(session, range.repository_id, sha)? {
                    let links = store::links_with_task_keys(session, &[commit.id])?;
                    found.push((
                        commit,
                        links.into_iter().map(|row| row.link).collect::<Vec<_>>(),
                    ));
                }
            }
            Ok(found)
        })?;
        let evidence = attempt_links::evidence(attempt_id, &range);
        let before = created;
        let outcome = app.runtime.decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut ids = stamp.ids();
                let mut writes = Vec::new();
                for (commit, links) in &commits {
                    let exists = links
                        .iter()
                        .any(|link| link.task_id == task_id && link.source == EXECUTION_CONTEXT);
                    if exists {
                        continue;
                    }
                    let (id, next) = ids.next();
                    ids = next;
                    writes.push(Write::Insert(Row::CommitTaskLink(CommitTaskLink {
                        id,
                        commit_id: commit.id,
                        task_id,
                        source: EXECUTION_CONTEXT.to_owned(),
                        confidence: Real(CONFIDENCE),
                        evidence: evidence.clone(),
                        attempt_id: Some(attempt_id),
                        created_by: ACTOR.to_owned(),
                        created_at: offset_time(stamp.at, writes.len()),
                    })));
                }
                let made = i64::try_from(writes.len()).unwrap_or(i64::MAX);
                let mut decision = Decision::new(made);
                decision.writes = writes.into_iter().collect();
                if before + made > 0 {
                    let (event_id, _) = ids.next();
                    decision.events = std::iter::once(
                        EventDraft::new(
                            stamp,
                            event_id,
                            Some(workspace_id),
                            attempt_id,
                            EventPayload::GitCommitsAttributed(GitCommitsAttributed {
                                links_created: before + made,
                                repository_id: Some(range.repository_id),
                            }),
                        )
                        // Python records an attempt's range against the attempt.
                        .with_entity_type(EntityType::Attempt)
                        .with_attempt(Some(attempt_id)),
                    )
                    .collect();
                }
                Ok(decision)
            },
        )?;
        created += outcome.reply;
    }
    Ok(created)
}
