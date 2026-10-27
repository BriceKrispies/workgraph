//! The worker dicts of `GET /workers`.

use wg_json::{Value, dict};
use wg_model::entity::queue::Worker;

use crate::convention::{TimeFormat, int, optional_timestamp, text, texts, timestamp};

/// The name of the worker dict; it has no OpenAPI component schema.
pub const NAME: &str = "WorkerOut";

/// The fields in the order the `/workers` route lists them (`kinds` is not on the wire).
pub const FIELDS: [&str; 11] = [
    "id",
    "hostname",
    "pid",
    "status",
    "concurrency",
    "current_jobs",
    "jobs_completed",
    "jobs_failed",
    "started_at",
    "heartbeat_at",
    "stopped_at",
];

/// A worker as the API returns it.
pub fn from_model(worker: &Worker) -> Value {
    dict!({
        "id": text(&worker.id),
        "hostname": text(&worker.hostname),
        "pid": int(worker.pid),
        "status": text(&worker.status),
        "concurrency": int(worker.concurrency),
        "current_jobs": texts(worker.current_jobs.iter().map(String::as_str)),
        "jobs_completed": int(worker.jobs_completed),
        "jobs_failed": int(worker.jobs_failed),
        "started_at": timestamp(worker.started_at, TimeFormat::Z),
        "heartbeat_at": timestamp(worker.heartbeat_at, TimeFormat::Z),
        "stopped_at": optional_timestamp(worker.stopped_at, TimeFormat::Z),
    })
}
