//! The pure half of the `worker` and `job` commands (`cli/commands/jobs.py`): the request
//! shaping (the status filter, the payload of an enqueue, the job reference) and the outputs the
//! services return (`--json` shapes and the counts the human views print).
//!
//! The outputs keep the key order of the Python dicts they copy. Datetimes are carried as
//! [`Value::DateTime`]: the JSON dialect of the CLI prints them as `isoformat()` and the human
//! views print `when()`.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json};
use wg_jobs::dispatch::JOB_KINDS;
use wg_json::{Value, dict};
use wg_model::entity::{Job, JobRun, Worker};
use wg_model::py_repr_str;
use wg_types::{Id, JobStatus, WgError};
use wg_wire::convention::{
    TimeFormat, counts, id, int, json_object, optional_id, optional_text, optional_timestamp, text,
    texts, timestamp,
};

/// `job list`'s default page size (`--limit`).
pub const LIST_DEFAULT_LIMIT: i64 = 50;

/// The statuses named by `--status`, parsed. `None` when one of them is not a status: the
/// listing then matches nothing, as Python's `status IN (...)` does.
pub fn status_filter(names: &[String]) -> Option<Vec<JobStatus>> {
    names
        .iter()
        .map(|name| JobStatus::parse(name))
        .collect::<Option<Vec<_>>>()
}

/// The message `job enqueue` fails with for a kind that has no handler, as Python prints it;
/// `None` for a kind in the handler table.
pub fn unknown_job_kind(kind: &str) -> Option<String> {
    if JOB_KINDS.contains(&kind) {
        return None;
    }
    let mut known = JOB_KINDS.to_vec();
    known.sort_unstable();
    Some(format!(
        "unknown job kind {}; known: {}",
        py_repr_str(kind),
        known.join(", ")
    ))
}

/// The payload of `job enqueue --payload`: a JSON object. Anything else is a validation error.
pub fn enqueue_payload(text: &str) -> Result<wg_model::entity::JsonObject, WgError> {
    let value: Json = serde_json::from_str(text)
        .map_err(|err| WgError::validation(format!("payload is not valid JSON: {err}")))?;
    match value {
        Json::Object(map) => Ok(map.into_iter().collect()),
        _ => Err(WgError::validation("payload must be a JSON object")),
    }
}

/// The reason a job reference names no job. A reference that is not an id names none either.
pub fn job_not_found(job_id: &str) -> WgError {
    WgError::not_found(format!("job {job_id} not found"))
}

/// The job id of a reference, or the not-found error for a reference that is not an id.
pub fn job_ref(job_id: &str) -> Result<Id, WgError> {
    wg_types::parse_id(job_id).map_err(|_| job_not_found(job_id))
}

/// One row of `job list`: the fields the listing prints.
pub fn job_item(job: &Job) -> Value {
    dict!({
        "id": id(job.id),
        "kind": text(&job.kind),
        "status": text(job.status.as_str()),
        "attempts": int(job.attempts),
        "max_attempts": int(job.max_attempts),
        "run_at": timestamp(job.run_at, TimeFormat::ModelZ),
        "last_error": optional_text(job.last_error.as_deref()),
        "error_kind": optional_text(job.error_kind.as_deref()),
        "task_id": optional_id(job.task_id),
        "attempt_id": optional_id(job.attempt_id),
        "created_at": timestamp(job.created_at, TimeFormat::ModelZ),
    })
}

/// `job list`: `{total, items}`.
pub fn job_list_out(jobs: &[Job], total: usize) -> Value {
    dict!({
        "total": int(i64::try_from(total).unwrap_or(i64::MAX)),
        "items": Value::List(jobs.iter().map(job_item).collect()),
    })
}

/// `job show`: the job's own fields, then its runs.
pub fn job_detail_out(job: &Job, runs: &[JobRun]) -> Value {
    dict!({
        "id": id(job.id),
        "kind": text(&job.kind),
        "status": text(job.status.as_str()),
        "payload": json_object(&job.payload),
        "result": job.result.as_ref().map_or(Value::Null, json_object),
        "attempts": int(job.attempts),
        "max_attempts": int(job.max_attempts),
        "run_at": timestamp(job.run_at, TimeFormat::ModelZ),
        "lease_owner": optional_text(job.lease_owner.as_deref()),
        "lease_expires_at": optional_timestamp(job.lease_expires_at, TimeFormat::ModelZ),
        "last_error": optional_text(job.last_error.as_deref()),
        "error_kind": optional_text(job.error_kind.as_deref()),
        "resource_keys": texts(job.resource_keys.iter().map(String::as_str)),
        "cancel_requested": Value::Bool(job.cancel_requested),
        "created_at": timestamp(job.created_at, TimeFormat::ModelZ),
        "finished_at": optional_timestamp(job.finished_at, TimeFormat::ModelZ),
        "runs": Value::List(runs.iter().map(run_out).collect()),
    })
}

/// One run of `job show`.
pub fn run_out(run: &JobRun) -> Value {
    dict!({
        "attempt": int(run.attempt_number),
        "worker": text(&run.worker_id),
        "status": text(&run.status),
        "error": optional_text(run.error.as_deref()),
        "started_at": timestamp(run.started_at, TimeFormat::ModelZ),
        "finished_at": optional_timestamp(run.finished_at, TimeFormat::ModelZ),
    })
}

/// `job stats`: the count of each status that occurs, in status-name order.
pub fn stats_out(stats: &BTreeMap<String, usize>) -> Value {
    let pairs: Vec<(String, i64)> = stats
        .iter()
        .map(|(status, count)| (status.clone(), i64::try_from(*count).unwrap_or(i64::MAX)))
        .collect();
    counts(&pairs)
}

/// One worker of `worker list`.
pub fn worker_item(worker: &Worker) -> Value {
    dict!({
        "id": text(&worker.id),
        "status": text(&worker.status),
        "pid": int(worker.pid),
        "concurrency": int(worker.concurrency),
        "current": texts(worker.current_jobs.iter().map(String::as_str)),
        "completed": int(worker.jobs_completed),
        "failed": int(worker.jobs_failed),
        "heartbeat_at": timestamp(worker.heartbeat_at, TimeFormat::ModelZ),
    })
}

/// `worker list`: a list of workers, newest first.
pub fn workers_out(workers: &[Worker]) -> Value {
    Value::List(workers.iter().map(worker_item).collect())
}

/// `worker recover`: the jobs whose leases expired, the workers marked lost and the worktrees
/// report (`worktrees` is the report of the worktree service, or its not-ported marker).
pub fn recovery_out(recovered: &[Id], lost: &[String], worktrees: Value) -> Value {
    dict!({
        "recovered_jobs": Value::List(recovered.iter().map(|job| id(*job)).collect()),
        "lost_workers": texts(lost.iter().map(String::as_str)),
        "worktrees": worktrees,
    })
}

/// `job cancel`: the job and the status it has now.
pub fn cancelled_out(job_id: &str, status: JobStatus) -> Value {
    dict!({
        "job_id": text(job_id),
        "status": text(status.as_str()),
    })
}

/// `job retry`: the job and the status it was re-queued as.
pub fn retried_out(job_id: &str) -> Value {
    dict!({
        "job_id": text(job_id),
        "status": text(JobStatus::Queued.as_str()),
    })
}

/// `job enqueue`: the id of the job (new or the one the key already names).
pub fn enqueued_out(job_id: Id) -> Value {
    dict!({ "job_id": id(job_id) })
}

/// `worker run`: what the worker did before it stopped.
pub fn worker_summary_out(worker_id: &str, completed: i64, failed: i64) -> Value {
    dict!({
        "worker": text(worker_id),
        "completed": int(completed),
        "failed": int(failed),
    })
}

/// `job compact`: a retention report as the maintenance pass returns it, keys in its order.
pub fn compact_out(report: &Map<String, Json>) -> Value {
    Value::Dict(
        report
            .iter()
            .map(|(key, value)| {
                (
                    wg_json::PyStr::from(key.as_str()),
                    wg_wire::convention::json_value(value),
                )
            })
            .collect(),
    )
}
