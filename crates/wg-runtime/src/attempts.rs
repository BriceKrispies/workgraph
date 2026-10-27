//! Reads of a task's attempts shared by the features.

use wg_model::entity::AgentAttempt;
use wg_store::Session;
use wg_types::{AttemptStatus, Id, WgError};

/// The latest succeeded attempt of `task` (`_latest_succeeded_attempt`).
pub fn latest_succeeded(
    session: &mut Session<'_>,
    task: Id,
) -> Result<Option<AgentAttempt>, WgError> {
    Ok(session
        .attempts_of_task(task, &[AttemptStatus::Succeeded])?
        .into_iter()
        .max_by_key(|attempt| attempt.number))
}
