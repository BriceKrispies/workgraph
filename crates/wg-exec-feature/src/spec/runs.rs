//! The run flow's writes as decisions over the task world (`RunService.start`, `cancel`, `retry`
//! and `rerun` of `orchestration/runs.py`): the queued attempt and its `agent_run` job, the
//! cancellation of a queued or running attempt, and the counter a retry bumps.
//!
//! The shell reads the task world, the task's attempts and the attempt's job, checks the provider,
//! hands them here and applies the decision in one transaction. Every decision replies with the
//! `AttemptOut` document the commands print.

use serde_json::{Map, Value, json};
use wg_exec::attempt::next_attempt_number;
use wg_exec::run::{
    BaselineEntry, CancelPlan, RERUN_NOTE, RUN_LEASE_MINUTES, RerunPlan, blocks_new_attempt,
    cancel_plan, cancel_releases_task, managed_start_plan, resource_keys,
};
use wg_jobs::queue::{NewJob, enqueue_as};
use wg_model::entity::{AgentAttempt, Job};
use wg_model::event::{AttemptCancelRequested, EventDraft, EventPayload};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Change, transition};
use wg_tasks_feature::spec::bodies::ClaimRead;
use wg_tasks_feature::spec::claims::claim_task;
use wg_types::{AttemptStatus, ErrorCode, Id, JobStatus, Permission, WgError};
use wg_wire::convention::{dict, text};
use wg_wire::dto::attempt;

use crate::spec::attempts::{Finish, NewAttempt, finish, new_attempt};
use crate::spec::prompt::PROMPT_VERSION;

/// The job kind a managed run is queued as.
pub const RUN_JOB: &str = "agent_run";

/// The priority a run job has when the caller names none.
pub const DEFAULT_PRIORITY: i64 = 50;

/// The reason `run cancel` records when the caller gives none.
pub const DEFAULT_CANCEL_REASON: &str = "cancelled by operator";

/// What a managed run start is asked for (the arguments of `RunService.start`).
#[derive(Debug, Clone)]
pub struct StartInputs {
    pub agent: String,
    /// The adapter type of the agent (`claude_code` or `command`).
    pub adapter: String,
    /// The provider's non-secret configuration (`config_snapshot()`).
    pub provider_snapshot: Map<String, Value>,
    pub model: Option<String>,
    pub instructions: Option<String>,
    /// The repository requests, as the config snapshot records them.
    pub repositories: Option<Value>,
    pub retry_of: Option<Id>,
    pub rerun_of: Option<Id>,
    pub parent_attempt: Option<Id>,
    pub priority: i64,
    /// `execution.isolation`.
    pub isolation: String,
}

/// What a run start is asked for besides the task (the keyword arguments of `RunService.start`).
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub repositories: Option<Value>,
    pub retry_of: Option<Id>,
    pub rerun_of: Option<Id>,
    pub parent_attempt: Option<Id>,
    pub priority: i64,
}

impl Default for StartRequest {
    fn default() -> Self {
        Self {
            agent: None,
            model: None,
            instructions: None,
            repositories: None,
            retry_of: None,
            rerun_of: None,
            parent_attempt: None,
            priority: DEFAULT_PRIORITY,
        }
    }
}

/// The start a retry asks for: the same agent and model, new instructions.
pub fn retry_request(previous: &AgentAttempt, instructions: Option<&str>) -> StartRequest {
    StartRequest {
        agent: Some(previous.provider.clone()),
        model: previous.model.clone(),
        instructions: instructions.map(str::to_owned),
        retry_of: Some(previous.id),
        ..StartRequest::default()
    }
}

fn entry_text(entry: &Map<String, Value>, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The checkout and the revision each baseline entry names (`source_checkout_path`, else
/// `checkout_path`; `sha`): what a rerun checks is still there.
pub fn baseline_sources(entries: &[Map<String, Value>]) -> Vec<(Option<String>, Option<String>)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry_text(entry, "source_checkout_path")
                    .or_else(|| entry_text(entry, "checkout_path")),
                entry_text(entry, "sha"),
            )
        })
        .collect()
}

/// The baseline entries with whether each revision is still available (in the order of `entries`).
pub fn baseline_facts(entries: &[Map<String, Value>], available: &[bool]) -> Vec<BaselineEntry> {
    entries
        .iter()
        .zip(available)
        .map(|(entry, found)| BaselineEntry {
            repository_id: entry_text(entry, "repository_id").unwrap_or_default(),
            repository: entry_text(entry, "repository").unwrap_or_default(),
            access: entry_text(entry, "access").unwrap_or_default(),
            sha: entry_text(entry, "sha"),
            revision_available: *found,
        })
        .collect()
}

/// The start a rerun asks for: the recorded agent, model and instructions, and the repositories of
/// the plan (none when the baseline had none).
pub fn rerun_request(previous: &AgentAttempt, plan: &RerunPlan) -> StartRequest {
    let snapshot = &previous.config_snapshot;
    let text = |key: &str| snapshot.get(key).and_then(Value::as_str).map(str::to_owned);
    let repositories: Vec<Value> = plan
        .repositories
        .iter()
        .map(|request| match &request.target_ref {
            Some(target) => json!({
                "repository_id": request.repository_id,
                "access": request.access,
                "target_ref": target,
            }),
            None => json!({
                "repository_id": request.repository_id,
                "access": request.access,
            }),
        })
        .collect();
    StartRequest {
        agent: text("agent"),
        model: text("model"),
        instructions: text("instructions"),
        repositories: (!repositories.is_empty()).then_some(Value::Array(repositories)),
        rerun_of: Some(previous.id),
        ..StartRequest::default()
    }
}

/// What `run rerun` prints: the new attempt, whether the baseline was reproduced and which
/// revisions were missing.
pub fn rerun_document(started: wg_json::Value, plan: &RerunPlan) -> wg_json::Value {
    let missing: Vec<wg_json::Value> = plan
        .missing
        .iter()
        .map(|found| {
            dict(vec![
                ("repository", text(&found.repository)),
                ("sha", text(&found.sha)),
            ])
        })
        .collect();
    dict(vec![
        ("attempt", started),
        (
            "reproduced_baseline",
            wg_json::Value::Bool(plan.reproduced_baseline()),
        ),
        ("missing_revisions", wg_json::Value::List(missing)),
        ("note", text(RERUN_NOTE)),
    ])
}

fn gone(what: &str, id: Id) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("{what} {id} not found"))
}

/// The same stamp with other entropy and the claim permission: the claim of a run start draws
/// its own ids, so they never meet the ids of the attempt and the job, and `run:start` is what
/// the caller was checked for, not `task:claim`.
fn claim_stamp(stamp: &Stamp) -> Stamp {
    Stamp {
        entropy: stamp.entropy.map(|byte| byte ^ 0x5a),
        permissions: stamp.permissions.insert(Permission::TaskClaim),
        ..stamp.clone()
    }
}

/// `RunService.start` after the provider check: claims a ready or failed task for the agent,
/// records a queued attempt and queues its `agent_run` job. The reply is the attempt.
pub fn start_run(
    world: &TaskWorld,
    attempts: &[AgentAttempt],
    task_id: Id,
    stamp: &Stamp,
    inputs: &StartInputs,
) -> Result<Decision<wg_json::Value>, WgError> {
    let task = world
        .tasks
        .task(task_id)
        .ok_or_else(|| gone("task", task_id))?;
    let first = managed_start_plan(
        &task.key.to_string(),
        task.status,
        None,
        inputs.parent_attempt.is_some(),
    )?;
    let claimed = if first.claim_first {
        claim_task(
            world,
            &claim_stamp(stamp),
            None,
            &task_id.to_string(),
            &ClaimRead {
                agent: Some(inputs.agent.clone()),
                lease_minutes: RUN_LEASE_MINUTES,
                expected_version: None,
            },
        )?
        .with_reply(())
    } else {
        Decision::new(())
    };
    let tasks = world.tasks.apply_all(claimed.writes.iter());
    let held = tasks.task(task_id).ok_or_else(|| gone("task", task_id))?;
    let active = attempts
        .iter()
        .find(|row| blocks_new_attempt(row.status) && row.parent_attempt_id.is_none())
        .map(|row| row.number);
    managed_start_plan(
        &held.key.to_string(),
        held.status,
        active,
        inputs.parent_attempt.is_some(),
    )?;
    let ids = stamp.ids();
    let (job_id, ids) = ids.next();
    let numbers: Vec<i64> = attempts.iter().map(|row| row.number).collect();
    let (row, created, _) = new_attempt(
        stamp,
        ids,
        held,
        NewAttempt {
            number: next_attempt_number(&numbers),
            status: AttemptStatus::Queued,
            provider: inputs.agent.clone(),
            adapter: inputs.adapter.clone(),
            agent_id: Some(format!("agent:{}", inputs.agent)),
            model: inputs.model.clone(),
            parent_attempt_id: inputs.parent_attempt,
            retry_of_attempt_id: inputs.retry_of,
            rerun_of_attempt_id: inputs.rerun_of,
            config_snapshot: config_snapshot(inputs),
            telemetry: Map::from_iter([("job_id".to_owned(), json!(job_id.to_string()))]),
        },
    );
    let mut job = NewJob::new(RUN_JOB);
    job.payload = Map::from_iter([("attempt_id".to_owned(), json!(row.id.to_string()))]);
    job.workspace_id = Some(held.workspace_id);
    job.priority = inputs.priority;
    job.task_id = Some(held.id);
    job.attempt_id = Some(row.id);
    job.max_attempts = 1;
    job.resource_keys = resource_keys(&inputs.agent, &inputs.isolation, &held.id.to_string());
    job.correlation_id = Some(stamp.correlation.clone());
    job.created_by = stamp.actor.to_string();
    job.timeout_seconds = None;
    let queued = enqueue_as(&QueueSlice::default(), &job, job_id, stamp);
    let reply = attempt::from_model(&row);
    Ok(claimed
        .then(Decision::new(()).write(Write::insert(row)).event(created))
        .then(queued.with_reply(()))
        .with_reply(reply))
}

/// The `config_snapshot` of a managed attempt: the provider's, then what this start chose.
fn config_snapshot(inputs: &StartInputs) -> Map<String, Value> {
    inputs
        .provider_snapshot
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .chain([
            ("agent".to_owned(), json!(inputs.agent)),
            ("model".to_owned(), json!(inputs.model)),
            ("instructions".to_owned(), json!(inputs.instructions)),
            (
                "repositories".to_owned(),
                inputs.repositories.clone().unwrap_or(Value::Null),
            ),
            ("prompt_version".to_owned(), json!(PROMPT_VERSION)),
            ("isolation".to_owned(), json!(inputs.isolation)),
        ])
        .collect()
}

/// `RunService.retry`'s first transaction: the previous attempt counts one more retry.
pub fn count_retry(previous: &AgentAttempt, stamp: &Stamp) -> Decision<()> {
    Decision::new(()).write(Write::update(AgentAttempt {
        retries: previous.retries + 1,
        updated_at: stamp.at,
        ..previous.clone()
    }))
}

/// `RunService.cancel`: a finished attempt is returned as it is; a queued one is cancelled on the
/// spot (and its queued job, and its task goes back to ready); a started one is asked to stop
/// through its attempt and job flags, and its worker concludes it.
pub fn cancel_run(
    world: &TaskWorld,
    attempt_row: &AgentAttempt,
    job: Option<&Job>,
    reason: &str,
    stamp: &Stamp,
) -> Result<Decision<wg_json::Value>, WgError> {
    let plan = cancel_plan(attempt_row.status, job.map(|row| row.status.as_str()));
    let flagged = AgentAttempt {
        cancel_requested: true,
        updated_at: stamp.at,
        ..attempt_row.clone()
    };
    let (event_id, ids) = stamp.ids().next();
    let requested = EventDraft::new(
        stamp,
        event_id,
        Some(attempt_row.workspace_id),
        attempt_row.id,
        EventPayload::AttemptCancelRequested(AttemptCancelRequested {
            reason: Some(reason.to_owned()),
        }),
    )
    .with_attempt(Some(attempt_row.id));
    match plan {
        CancelPlan::Unchanged => Ok(Decision::new(attempt::from_model(attempt_row))),
        CancelPlan::RequestStop { flag_job } => {
            let flagged_job = job.filter(|_| flag_job).map(|row| Job {
                cancel_requested: true,
                updated_at: stamp.at,
                ..row.clone()
            });
            let reply = attempt::from_model(&flagged);
            let decision = Decision::new(()).write(Write::update(flagged));
            let decision = match flagged_job {
                Some(row) => decision.write(Write::update(row)),
                None => decision,
            };
            Ok(decision.event(requested).with_reply(reply))
        }
        CancelPlan::CancelQueued { cancel_job } => {
            let cancelled_job = job.filter(|_| cancel_job).map(|row| Job {
                status: JobStatus::Cancelled,
                cancel_requested: true,
                finished_at: Some(stamp.at),
                updated_at: stamp.at,
                ..row.clone()
            });
            let (finish_id, ids) = ids.next();
            let (finished, ended) = finish(
                stamp,
                finish_id,
                &flagged,
                AttemptStatus::Cancelled,
                &Finish {
                    exit_reason: Some("cancelled".to_owned()),
                    error: Some(reason.to_owned()),
                    ..Finish::default()
                },
            )?
            .ok_or_else(|| gone("attempt", attempt_row.id))?;
            let reply = attempt::from_model(&finished);
            let base = Decision::new(())
                .write(Write::update(finished))
                .event(ended);
            let base = match cancelled_job {
                Some(row) => base.write(Write::update(row)),
                None => base,
            };
            let released = world
                .tasks
                .task(attempt_row.task_id)
                .filter(|task| cancel_releases_task(task.status))
                .map(|task| {
                    transition(
                        &world.tasks,
                        &world.stacks,
                        stamp,
                        ids,
                        task.id,
                        wg_types::TaskStatus::Ready,
                        Change {
                            reason: Some(reason),
                            ..Change::default()
                        },
                    )
                })
                .transpose()?;
            let base = match released {
                Some(moved) => base.then(moved.decision),
                None => base,
            };
            Ok(base.event(requested).with_reply(reply))
        }
    }
}
