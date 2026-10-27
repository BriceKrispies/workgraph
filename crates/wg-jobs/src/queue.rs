//! The durable job queue (`jobs/queue.py`) as pure decisions over a [`QueueSlice`].
//!
//! * Claims: the runnable jobs in priority, run-time, creation order; the first one whose resource
//!   keys all have capacity is taken, as the Python claim does within its first 100 candidates.
//! * Leases: a claim sets a lease expiring `lease_seconds` after `stamp.at`; only the lease owner
//!   may heartbeat, complete, fail or cancel; a lease expiring strictly before `stamp.at` is
//!   recovered.
//! * Retries: exponential backoff `base * 2^(attempt-1)` capped at one hour; then the dead letter.
//! * Idempotency: a job whose key is already present is returned instead of inserted.
//!
//! Every function takes the slice the shell loaded and the stamp of the request, and returns the
//! writes the shell applies in the same transaction. No function reads a clock or draws ids except
//! through the stamp.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use rpds::VectorSync;
use wg_model::decision::Decision;
use wg_model::entity::{Job, JobResourceClaim, JobRun, JsonObject, Real};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::write::{RowKey, Write};
use wg_types::{Correlation, Id, JobStatus, Timestamp, WgError};

use crate::limits::LimitSettings;
use crate::{shift, truncate_chars};

/// Statuses a job holds while it still counts as pending work (`ACTIVE_STATUSES`).
pub const ACTIVE_STATUSES: [JobStatus; 2] = [JobStatus::Queued, JobStatus::Running];

/// Candidates considered by one claim, in queue order (the Python `limit(100)`).
pub const CLAIM_CANDIDATES: usize = 100;

/// Longest `last_error` kept on the job row, in characters.
pub const ERROR_TEXT_LIMIT: usize = 20_000;

/// Longest error kept on a run row, in characters.
pub const RUN_ERROR_LIMIT: usize = 5_000;

/// Upper bound of one backoff delay, in seconds.
pub const BACKOFF_CAP_SECONDS: f64 = 3600.0;

const ERROR_KIND_LEASE_EXPIRED: &str = "lease_expired";

/// The arguments of an enqueue, with the defaults of the Python `JobQueue.enqueue`.
#[derive(Debug, Clone, PartialEq)]
pub struct NewJob {
    pub kind: String,
    pub payload: JsonObject,
    pub workspace_id: Option<Id>,
    pub priority: i64,
    /// `None` runs the job at `stamp.at`.
    pub run_at: Option<Timestamp>,
    pub max_attempts: i64,
    pub backoff_base_seconds: f64,
    pub timeout_seconds: Option<f64>,
    pub idempotency_key: Option<String>,
    pub resource_keys: Vec<String>,
    pub correlation_id: Option<Correlation>,
    pub causation_id: Option<Id>,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub created_by: String,
}

impl NewJob {
    /// A job of `kind` with the queue's defaults: priority 100, five attempts, base 2 seconds.
    pub fn new(kind: &str) -> Self {
        NewJob {
            kind: kind.to_owned(),
            payload: JsonObject::new(),
            workspace_id: None,
            priority: 100,
            run_at: None,
            max_attempts: 5,
            backoff_base_seconds: 2.0,
            timeout_seconds: None,
            idempotency_key: None,
            resource_keys: Vec::new(),
            correlation_id: None,
            causation_id: None,
            task_id: None,
            attempt_id: None,
            created_by: "system".to_owned(),
        }
    }
}

/// What a worker receives from a successful claim (`ClaimedJob`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedJob {
    pub id: Id,
    pub kind: String,
    pub payload: JsonObject,
    /// The attempt number of this claim, counting from 1.
    pub attempt: i64,
    pub max_attempts: i64,
    pub workspace_id: Option<Id>,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub correlation_id: Option<Correlation>,
    pub causation_id: Option<Id>,
    pub timeout_seconds: Option<f64>,
    pub lease_expires_at: Timestamp,
}

/// Filters and paging of a job listing; the default lists every job, newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobFilter<'a> {
    pub statuses: &'a [JobStatus],
    pub kind: Option<&'a str>,
    pub workspace_id: Option<Id>,
    pub task_id: Option<Id>,
    pub offset: usize,
    /// `None` lists every match.
    pub limit: Option<usize>,
}

/// Seconds of backoff before the retry that follows `attempt` failed attempts:
/// `base * 2^(attempt-1)`, never below zero attempts, capped at `cap_seconds`.
pub fn backoff_delay(base_seconds: f64, attempt: i64, cap_seconds: f64) -> f64 {
    if base_seconds == 0.0 {
        return 0.0;
    }
    let exponent = (attempt - 1).max(0);
    let factor = 2f64.powi(i32::try_from(exponent).unwrap_or(i32::MAX));
    (base_seconds * factor).min(cap_seconds)
}

/// Inserts a job, or returns the id of the job already holding its idempotency key (in any status).
pub fn enqueue(slice: &QueueSlice, job: &NewJob, stamp: &Stamp) -> Decision<Id> {
    enqueue_as(slice, job, stamp.id(0), stamp)
}

/// [`enqueue`] with the id of a new job given by the caller. A decision that enqueues several jobs
/// takes one id per job from its stamp, since every job of a stamp otherwise gets the same id.
pub fn enqueue_as(slice: &QueueSlice, job: &NewJob, id: Id, stamp: &Stamp) -> Decision<Id> {
    let existing = job.idempotency_key.as_deref().and_then(|key| {
        slice
            .jobs
            .values()
            .find(|row| row.idempotency_key.as_deref() == Some(key))
    });
    match existing {
        Some(row) => Decision::new(row.id),
        None => {
            let row = Job {
                id,
                workspace_id: job.workspace_id,
                kind: job.kind.clone(),
                status: JobStatus::Queued,
                priority: job.priority,
                payload: job.payload.clone(),
                result: None,
                run_at: job.run_at.unwrap_or(stamp.at),
                attempts: 0,
                max_attempts: job.max_attempts,
                backoff_base_seconds: Real(job.backoff_base_seconds),
                timeout_seconds: job.timeout_seconds.map(Real),
                lease_owner: None,
                lease_expires_at: None,
                heartbeat_at: None,
                started_at: None,
                finished_at: None,
                cancel_requested: false,
                idempotency_key: job.idempotency_key.clone(),
                resource_keys: sorted_unique(&job.resource_keys),
                last_error: None,
                error_kind: None,
                correlation_id: job.correlation_id.clone(),
                causation_id: job.causation_id,
                task_id: job.task_id,
                attempt_id: job.attempt_id,
                created_by: job.created_by.clone(),
                created_at: stamp.at,
                updated_at: stamp.at,
            };
            Decision::new(id).write(Write::insert(row))
        }
    }
}

/// Claims the next runnable job for `worker` (`JobQueue.claim`). `kinds` empty means every kind.
/// The reply is `None` when nothing runnable has capacity.
pub fn claim(
    slice: &QueueSlice,
    worker: &str,
    kinds: &[&str],
    lease_seconds: i64,
    limits: &LimitSettings,
    stamp: &Stamp,
) -> Decision<Option<ClaimedJob>> {
    let now = stamp.at;
    let chosen = runnable(slice, kinds, now)
        .into_iter()
        .take(CLAIM_CANDIDATES)
        .find(|job| {
            job.resource_keys
                .iter()
                .all(|key| has_capacity(slice, limits, key))
        });
    match chosen {
        None => Decision::new(None),
        Some(job) => {
            let lease_expires_at = shift(now, lease_seconds as f64);
            let attempt = job.attempts + 1;
            let row = Job {
                status: JobStatus::Running,
                attempts: attempt,
                lease_owner: Some(worker.to_owned()),
                lease_expires_at: Some(lease_expires_at),
                heartbeat_at: Some(now),
                started_at: Some(now),
                updated_at: now,
                ..job.clone()
            };
            let run = JobRun {
                id: stamp.id(0),
                job_id: job.id,
                attempt_number: attempt,
                worker_id: worker.to_owned(),
                status: "running".to_owned(),
                error: None,
                started_at: now,
                finished_at: None,
            };
            let claims = job.resource_keys.iter().map(|key| {
                Write::insert(JobResourceClaim {
                    job_id: job.id,
                    resource_key: key.clone(),
                })
            });
            let claimed = ClaimedJob {
                id: job.id,
                kind: job.kind.clone(),
                payload: job.payload.clone(),
                attempt,
                max_attempts: job.max_attempts,
                workspace_id: job.workspace_id,
                task_id: job.task_id,
                attempt_id: job.attempt_id,
                correlation_id: job.correlation_id.clone(),
                causation_id: job.causation_id,
                timeout_seconds: job.timeout_seconds.map(|real| real.0),
                lease_expires_at,
            };
            decide(
                Some(claimed),
                [Write::update(row), Write::insert(run)]
                    .into_iter()
                    .chain(claims),
            )
        }
    }
}

/// The id of the first runnable job, without claiming it or checking resource limits
/// (`JobQueue.claim_peek`).
pub fn claim_peek(slice: &QueueSlice, kinds: &[&str], stamp: &Stamp) -> Option<Id> {
    runnable(slice, kinds, stamp.at).first().map(|job| job.id)
}

/// Extends the lease of the running job held by `worker`. The reply is whether cancellation has
/// been requested. A job not leased to `worker` is a conflict (also when it does not exist).
pub fn heartbeat(
    slice: &QueueSlice,
    job_id: Id,
    worker: &str,
    lease_seconds: i64,
    stamp: &Stamp,
) -> Result<Decision<bool>, WgError> {
    let job = slice
        .job(job_id)
        .filter(|job| is_leased_to(job, worker))
        .ok_or_else(|| {
            WgError::conflict(format!("job {job_id} is no longer leased by {worker}"))
        })?;
    let now = stamp.at;
    let row = Job {
        heartbeat_at: Some(now),
        lease_expires_at: Some(shift(now, lease_seconds as f64)),
        updated_at: now,
        ..job.clone()
    };
    Ok(Decision::new(job.cancel_requested).write(Write::update(row)))
}

/// Marks a leased job succeeded with `result` (an empty object when absent).
pub fn complete(
    slice: &QueueSlice,
    job_id: Id,
    worker: &str,
    result: Option<JsonObject>,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let job = owned(slice, job_id, worker)?;
    let now = stamp.at;
    let row = Job {
        status: JobStatus::Succeeded,
        result: Some(result.unwrap_or_default()),
        finished_at: Some(now),
        lease_owner: None,
        lease_expires_at: None,
        updated_at: now,
        ..job.clone()
    };
    let finished = finish_run(slice, &row, JobStatus::Succeeded, None, now);
    Ok(decide(
        (),
        release(slice, job_id)
            .into_iter()
            .chain([Write::update(row)])
            .chain(finished),
    ))
}

/// Records a failure of the job leased to `worker`. The reply is the status the job moved to:
/// `queued` for a retry, `dead` or `cancelled` otherwise.
pub fn fail(
    slice: &QueueSlice,
    job_id: Id,
    worker: &str,
    error: &str,
    retryable: bool,
    error_kind: Option<&str>,
    stamp: &Stamp,
) -> Result<Decision<JobStatus>, WgError> {
    let job = owned(slice, job_id, worker)?;
    let (status, writes) = record_failure(slice, job, error, retryable, error_kind, stamp.at);
    Ok(decide(status, writes))
}

/// Ends the job leased to `worker` as cancelled, with `reason` as its error.
pub fn mark_cancelled(
    slice: &QueueSlice,
    job_id: Id,
    worker: &str,
    reason: &str,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let job = owned(slice, job_id, worker)?;
    let now = stamp.at;
    let row = Job {
        status: JobStatus::Cancelled,
        finished_at: Some(now),
        last_error: Some(reason.to_owned()),
        lease_owner: None,
        lease_expires_at: None,
        updated_at: now,
        ..job.clone()
    };
    let finished = finish_run(slice, &row, JobStatus::Cancelled, Some(reason), now);
    Ok(decide(
        (),
        release(slice, job_id)
            .into_iter()
            .chain([Write::update(row)])
            .chain(finished),
    ))
}

/// Re-queues (or dead-letters) every running job whose lease expired strictly before `stamp.at`.
/// The reply lists the recovered job ids in key order.
pub fn recover_expired(slice: &QueueSlice, stamp: &Stamp) -> Decision<Vec<Id>> {
    let now = stamp.at;
    let expired: Vec<&Job> = slice
        .jobs
        .values()
        .filter(|job| {
            job.status == JobStatus::Running
                && job.lease_expires_at.is_some_and(|expires| expires < now)
        })
        .collect();
    let recovered: Vec<(Id, Vec<Write>)> = expired
        .into_iter()
        .map(|job| {
            let owner = job.lease_owner.as_deref().unwrap_or("None");
            let message = format!("lease expired (worker {owner} stopped heartbeating)");
            let (_, writes) = record_failure(
                slice,
                job,
                &message,
                true,
                Some(ERROR_KIND_LEASE_EXPIRED),
                now,
            );
            (job.id, writes)
        })
        .collect();
    let ids = recovered.iter().map(|(id, _)| *id).collect();
    decide(ids, recovered.into_iter().flat_map(|(_, writes)| writes))
}

/// Cancels a job (`JobQueue.cancel`). A queued job becomes cancelled at once; a running one only
/// has `cancel_requested` set, and its worker learns of it on the next heartbeat. Other statuses
/// are reported unchanged.
pub fn cancel(
    slice: &QueueSlice,
    job_id: Id,
    stamp: &Stamp,
) -> Result<Decision<JobStatus>, WgError> {
    let job = slice
        .job(job_id)
        .ok_or_else(|| WgError::not_found(format!("job {job_id} not found")))?;
    let now = stamp.at;
    match job.status {
        JobStatus::Queued => {
            let row = Job {
                status: JobStatus::Cancelled,
                finished_at: Some(now),
                cancel_requested: true,
                updated_at: now,
                ..job.clone()
            };
            Ok(decide(JobStatus::Cancelled, [Write::update(row)]))
        }
        JobStatus::Running => {
            let row = Job {
                cancel_requested: true,
                updated_at: now,
                ..job.clone()
            };
            Ok(decide(JobStatus::Running, [Write::update(row)]))
        }
        other => Ok(decide(other, Vec::new())),
    }
}

/// Gives a dead, failed or cancelled job a fresh retry budget and makes it runnable now
/// (`JobQueue.retry_dead`).
pub fn retry_dead(slice: &QueueSlice, job_id: Id, stamp: &Stamp) -> Result<Decision<()>, WgError> {
    let job = slice
        .job(job_id)
        .ok_or_else(|| WgError::not_found(format!("job {job_id} not found")))?;
    if !matches!(
        job.status,
        JobStatus::Dead | JobStatus::Cancelled | JobStatus::Failed
    ) {
        return Err(WgError::conflict(format!(
            "job {job_id} is {}; only dead/cancelled jobs can be retried",
            job.status
        )));
    }
    let now = stamp.at;
    let row = Job {
        status: JobStatus::Queued,
        max_attempts: job.attempts + job.max_attempts.max(1),
        run_at: now,
        cancel_requested: false,
        finished_at: None,
        updated_at: now,
        ..job.clone()
    };
    Ok(decide((), [Write::update(row)]))
}

/// The job with `job_id`.
pub fn get(slice: &QueueSlice, job_id: Id) -> Result<&Job, WgError> {
    slice
        .job(job_id)
        .ok_or_else(|| WgError::not_found(format!("job {job_id} not found")))
}

/// The jobs matching `filter`, newest first, with the number of matches before paging
/// (`JobQueue.list`).
pub fn list<'a>(slice: &'a QueueSlice, filter: &JobFilter<'_>) -> (Vec<&'a Job>, usize) {
    let matching: BTreeMap<(Reverse<Timestamp>, Id), &Job> = slice
        .jobs
        .values()
        .filter(|job| matches_filter(job, filter))
        .map(|job| ((Reverse(job.created_at), job.id), job))
        .collect();
    let total = matching.len();
    let page = matching
        .into_values()
        .skip(filter.offset)
        .take(filter.limit.unwrap_or(usize::MAX))
        .collect();
    (page, total)
}

/// The attempts of a job in start order (`JobQueue.runs`).
pub fn runs(slice: &QueueSlice, job_id: Id) -> Vec<&JobRun> {
    slice
        .runs_of(job_id)
        .map(|run| ((run.started_at, run.attempt_number, run.id), run))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// Job counts by status, for the statuses that occur (`JobQueue.stats`).
pub fn stats(slice: &QueueSlice, workspace_id: Option<Id>) -> BTreeMap<String, usize> {
    const STATUSES: [JobStatus; 6] = [
        JobStatus::Queued,
        JobStatus::Running,
        JobStatus::Succeeded,
        JobStatus::Failed,
        JobStatus::Dead,
        JobStatus::Cancelled,
    ];
    let visible: Vec<&Job> = slice
        .jobs
        .values()
        .filter(|job| {
            workspace_id.is_none_or(|ws| job.workspace_id.is_none() || job.workspace_id == Some(ws))
        })
        .collect();
    STATUSES
        .into_iter()
        .filter_map(|status| {
            let count = visible.iter().filter(|job| job.status == status).count();
            (count > 0).then(|| (status.as_str().to_owned(), count))
        })
        .collect()
}

/// Runnable jobs in claim order: priority, run time, creation time, then id.
fn runnable<'a>(slice: &'a QueueSlice, kinds: &[&str], now: Timestamp) -> Vec<&'a Job> {
    slice
        .jobs
        .values()
        .filter(|job| {
            job.status == JobStatus::Queued
                && job.run_at <= now
                && !job.cancel_requested
                && (kinds.is_empty() || kinds.contains(&job.kind.as_str()))
        })
        .map(|job| ((job.priority, job.run_at, job.created_at, job.id), job))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// Running jobs holding `key` are fewer than the key's limit (every key without a limit has room).
fn has_capacity(slice: &QueueSlice, limits: &LimitSettings, key: &str) -> bool {
    limits
        .limit_for(key)
        .is_none_or(|limit| held(slice, key) < limit)
}

fn held(slice: &QueueSlice, key: &str) -> i64 {
    let count = slice
        .claims
        .values()
        .filter(|claim| {
            claim.resource_key == key
                && slice
                    .job(claim.job_id)
                    .is_some_and(|job| job.status == JobStatus::Running)
        })
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

fn is_leased_to(job: &Job, worker: &str) -> bool {
    job.status == JobStatus::Running && job.lease_owner.as_deref() == Some(worker)
}

/// The job, when it is running and leased to `worker` (`_owned`).
fn owned<'a>(slice: &'a QueueSlice, job_id: Id, worker: &str) -> Result<&'a Job, WgError> {
    let job = slice
        .job(job_id)
        .ok_or_else(|| WgError::not_found(format!("job {job_id} not found")))?;
    if is_leased_to(job, worker) {
        Ok(job)
    } else {
        Err(WgError::conflict(format!(
            "job {job_id} is not leased by {worker} (status={})",
            job.status
        )))
    }
}

/// The writes of a failure (`_record_failure`): a retry is queued after its backoff, otherwise the
/// job is dead, or cancelled when cancellation was requested. The reply is the new status.
fn record_failure(
    slice: &QueueSlice,
    job: &Job,
    error: &str,
    retryable: bool,
    error_kind: Option<&str>,
    now: Timestamp,
) -> (JobStatus, Vec<Write>) {
    let retry = retryable && job.attempts < job.max_attempts && !job.cancel_requested;
    let (status, run_at, finished_at) = if retry {
        let delay = backoff_delay(
            job.backoff_base_seconds.0,
            job.attempts,
            BACKOFF_CAP_SECONDS,
        );
        (JobStatus::Queued, shift(now, delay), job.finished_at)
    } else {
        let terminal = if job.cancel_requested {
            JobStatus::Cancelled
        } else {
            JobStatus::Dead
        };
        (terminal, job.run_at, Some(now))
    };
    let row = Job {
        status,
        run_at,
        finished_at,
        last_error: Some(truncate_chars(error, ERROR_TEXT_LIMIT)),
        error_kind: error_kind.map(str::to_owned),
        lease_owner: None,
        lease_expires_at: None,
        updated_at: now,
        ..job.clone()
    };
    let run_status = if retry { JobStatus::Failed } else { status };
    let finished = finish_run(slice, &row, run_status, Some(error), now);
    let writes = release(slice, job.id)
        .into_iter()
        .chain([Write::update(row)])
        .chain(finished)
        .collect();
    (status, writes)
}

/// Deletes the resource claims of a job.
fn release(slice: &QueueSlice, job_id: Id) -> Vec<Write> {
    slice
        .claims_of(job_id)
        .map(|claim| {
            Write::Delete(RowKey::JobResourceClaim {
                job_id,
                resource_key: claim.resource_key.clone(),
            })
        })
        .collect()
}

/// The run of the job's current attempt, closed with `status` and `error` (`_finish_run`).
fn finish_run(
    slice: &QueueSlice,
    job: &Job,
    status: JobStatus,
    error: Option<&str>,
    now: Timestamp,
) -> Option<Write> {
    let run = slice
        .runs_of(job.id)
        .filter(|run| run.attempt_number == job.attempts)
        .max_by_key(|run| run.started_at)?;
    let error = error
        .filter(|text| !text.is_empty())
        .map(|text| truncate_chars(text, RUN_ERROR_LIMIT));
    Some(Write::update(JobRun {
        status: status.as_str().to_owned(),
        error,
        finished_at: Some(now),
        ..run.clone()
    }))
}

fn matches_filter(job: &Job, filter: &JobFilter<'_>) -> bool {
    (filter.statuses.is_empty() || filter.statuses.contains(&job.status))
        && filter.kind.is_none_or(|kind| job.kind == kind)
        && filter
            .workspace_id
            .is_none_or(|ws| job.workspace_id.is_none() || job.workspace_id == Some(ws))
        && filter.task_id.is_none_or(|task| job.task_id == Some(task))
}

fn decide<R>(reply: R, writes: impl IntoIterator<Item = Write>) -> Decision<R> {
    writes
        .into_iter()
        .fold(Decision::new(reply), Decision::write)
}

fn sorted_unique(keys: &[String]) -> VectorSync<String> {
    keys.iter()
        .cloned()
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}
