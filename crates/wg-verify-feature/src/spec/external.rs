//! Pure: the acceptance a tracker's done issue implies (`VerificationService.record_external_acceptance`).
//!
//! The decision writes the acceptance row (`mode` `external`, `reason` `external:<connection>`, no
//! verification run and no manifest), the `task.accepted` event, and the `compute_change_surface`
//! job the acceptance queues. The reply is the event id: the caller moves the task to `accepted`
//! and chains that move after this decision, caused by the event.

use serde_json::json;
use wg_jobs::queue::{NewJob, enqueue};
use wg_model::entity::{AgentAttempt, Task, TaskAcceptance};
use wg_model::event::{EventDraft, EventPayload, TaskAccepted};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_tasks::accept::substamp;
use wg_types::Id;

/// The acceptance of `task` implied by its tracker issue `connection` being done. `latest_attempt`
/// is the task's newest succeeded attempt, if it has one.
pub fn record_external_acceptance(
    stamp: &Stamp,
    task: &Task,
    latest_attempt: Option<&AgentAttempt>,
    connection: &str,
) -> Decision<Id> {
    let ([acceptance_id, event_id], ids) = stamp.ids().take::<2>();
    let attempt = latest_attempt.map(|found| found.id);
    let reason = format!("external:{connection}");
    let acceptance = TaskAcceptance {
        id: acceptance_id,
        task_id: task.id,
        attempt_id: attempt,
        verification_run_id: None,
        decision: "accepted".to_owned(),
        mode: "external".to_owned(),
        reason: reason.clone(),
        actor: stamp.actor.to_string(),
        manifest_id: None,
        created_at: stamp.at,
    };
    let recorded = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskAccepted(TaskAccepted {
            mode: "external".to_owned(),
            reason,
            verification_run_id: None,
            manifest_id: None,
        }),
    )
    .with_attempt(attempt);
    let queued = surface_job(stamp, task, acceptance_id, event_id, ids.position());
    Decision::new(())
        .write(Write::insert(acceptance))
        .event(recorded)
        .then(queued)
        .with_reply(event_id)
}

/// `_enqueue_surface`: the `compute_change_surface` job an acceptance queues. Its idempotency key
/// names the acceptance, so no stored job can already hold it.
fn surface_job(
    stamp: &Stamp,
    task: &Task,
    acceptance: Id,
    caused_by: Id,
    position: u32,
) -> Decision<()> {
    let mut job = NewJob::new("compute_change_surface");
    job.payload = json!({"task_id": task.id.to_string()})
        .as_object()
        .cloned()
        .unwrap_or_default();
    job.workspace_id = Some(task.workspace_id);
    job.task_id = Some(task.id);
    job.idempotency_key = Some(format!("surface:{}:{acceptance}", task.id));
    job.correlation_id = Some(stamp.correlation.clone());
    job.causation_id = Some(caused_by);
    let queued = enqueue(&QueueSlice::default(), &job, &substamp(stamp, position));
    queued
        .writes
        .iter()
        .fold(Decision::new(()), |decision, write| {
            decision.write(write.clone())
        })
}
