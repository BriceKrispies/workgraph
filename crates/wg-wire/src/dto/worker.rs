//! The worker dicts of `GET /workers`.

use rpds::VectorSync;
use wg_model::entity::queue::Worker;
use wg_types::Timestamp;

use crate::convention::z_text;
use crate::wire_dto;

wire_dto! {
    /// A worker as the API returns it: a dict with no OpenAPI component schema, in the order the
    /// `/workers` route lists the fields (`kinds` is not on the wire).
    pub struct WorkerOut<'a>("WorkerOut") dict from_model(w: &'a Worker) {
        id: &'a str = &w.id,
        hostname: &'a str = &w.hostname,
        pid: i64 = w.pid,
        status: &'a str = &w.status,
        concurrency: i64 = w.concurrency,
        current_jobs: &'a VectorSync<String> = &w.current_jobs,
        jobs_completed: i64 = w.jobs_completed,
        jobs_failed: i64 = w.jobs_failed,
        #[serde(serialize_with = "z_text")]
        started_at: Timestamp = w.started_at,
        #[serde(serialize_with = "z_text")]
        heartbeat_at: Timestamp = w.heartbeat_at,
        #[serde(serialize_with = "z_text")]
        stopped_at: Option<Timestamp> = w.stopped_at,
    }
}
