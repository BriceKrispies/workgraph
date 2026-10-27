//! Shell: the run reads of the `run` commands (`AgentRunService.get`): an attempt's state, the
//! status of its `agent_run` job and its recent progress events.

use serde_json::Value as Json;
use wg_jobs::queue::JobFilter;
use wg_json::Value;
use wg_model::entity::{AgentAttempt, Job};
use wg_model::py_repr_str;
use wg_runtime::App;
use wg_store::{EventQuery, Session};
use wg_types::{WgError, parse_id};

use crate::spec::cli_run::run_document;

/// The attempt `reference` names (`resolve.attempt`): by its id, or `not_found` when none matches.
pub(crate) fn find_attempt(
    session: &mut Session<'_>,
    reference: &str,
) -> Result<AgentAttempt, WgError> {
    let found = match parse_id(reference) {
        Ok(id) => session.attempt_by_id(id)?,
        Err(_) => None,
    };
    found.ok_or_else(|| {
        WgError::not_found(format!("attempt {} not found", py_repr_str(reference)))
            .detail("ref", Json::String(reference.to_owned()))
    })
}

/// The `agent_run` job of an attempt, if it has one (`select(Job).where(attempt_id, kind)`).
pub(crate) fn agent_job(
    session: &mut Session<'_>,
    attempt: &AgentAttempt,
) -> Result<Option<Job>, WgError> {
    let (jobs, _) = session.list_jobs(&JobFilter {
        statuses: &[],
        kind: Some("agent_run"),
        workspace_id: None,
        task_id: Some(attempt.task_id),
        offset: 0,
        limit: None,
    })?;
    Ok(jobs
        .into_iter()
        .find(|job| job.attempt_id == Some(attempt.id)))
}

/// `AgentRunService.get`: the `RunOut` document of the attempt `reference` names, with at most
/// `events` recent progress events.
pub fn get(app: &mut App, reference: &str, events: i64) -> Result<Value, WgError> {
    app.runtime.store.read(|session| {
        let attempt = find_attempt(session, reference)?;
        let job = agent_job(session, &attempt)?;
        let recent = session.query_events(&EventQuery {
            attempt_id: Some(attempt.id),
            limit: events,
            descending: true,
            ..EventQuery::default()
        })?;
        Ok(run_document(&attempt, job.as_ref(), &recent))
    })
}
