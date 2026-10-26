//! Worker registration and liveness (the worker methods of `jobs/queue.py`).
//!
//! A worker row is written when it starts, refreshed on every heartbeat, marked stopped when it
//! shuts down, and marked lost by maintenance once its heartbeat is older than the stale window.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use rpds::VectorSync;
use wg_model::decision::Decision;
use wg_model::entity::Worker;
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::write::Write;

use crate::shift;

pub const STATUS_RUNNING: &str = "running";
pub const STATUS_STOPPED: &str = "stopped";
pub const STATUS_LOST: &str = "lost";

/// Listing cap of `JobQueue.workers` (the Python `limit(50)`).
pub const WORKER_LISTING_LIMIT: usize = 50;

/// The arguments of a worker registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorker {
    pub id: String,
    pub hostname: String,
    pub pid: i64,
    pub concurrency: i64,
    pub kinds: Vec<String>,
}

/// Inserts the worker, or restarts an existing row as running (`register_worker`).
pub fn register_worker(slice: &QueueSlice, worker: &NewWorker, stamp: &Stamp) -> Decision<()> {
    let now = stamp.at;
    match slice.worker(&worker.id) {
        None => Decision::new(()).write(Write::insert(Worker {
            id: worker.id.clone(),
            hostname: worker.hostname.clone(),
            pid: worker.pid,
            status: STATUS_RUNNING.to_owned(),
            concurrency: worker.concurrency,
            kinds: worker.kinds.iter().cloned().collect::<VectorSync<String>>(),
            current_jobs: VectorSync::new_sync(),
            jobs_completed: 0,
            jobs_failed: 0,
            started_at: now,
            heartbeat_at: now,
            stopped_at: None,
        })),
        Some(existing) => Decision::new(()).write(Write::update(Worker {
            status: STATUS_RUNNING.to_owned(),
            stopped_at: None,
            heartbeat_at: now,
            ..existing.clone()
        })),
    }
}

/// Records a worker heartbeat with its current jobs and counters (`worker_heartbeat`). A worker
/// that is not registered is left alone.
pub fn worker_heartbeat(
    slice: &QueueSlice,
    worker_id: &str,
    current_jobs: &[String],
    completed: i64,
    failed: i64,
    stamp: &Stamp,
) -> Decision<()> {
    match slice.worker(worker_id) {
        None => Decision::new(()),
        Some(existing) => Decision::new(()).write(Write::update(Worker {
            heartbeat_at: stamp.at,
            current_jobs: current_jobs.iter().cloned().collect::<VectorSync<String>>(),
            jobs_completed: completed,
            jobs_failed: failed,
            ..existing.clone()
        })),
    }
}

/// Marks a worker stopped and clears its current jobs (`stop_worker`).
pub fn stop_worker(slice: &QueueSlice, worker_id: &str, stamp: &Stamp) -> Decision<()> {
    match slice.worker(worker_id) {
        None => Decision::new(()),
        Some(existing) => Decision::new(()).write(Write::update(Worker {
            status: STATUS_STOPPED.to_owned(),
            stopped_at: Some(stamp.at),
            current_jobs: VectorSync::new_sync(),
            ..existing.clone()
        })),
    }
}

/// Marks every running worker whose heartbeat is strictly older than `stale_after_seconds` as
/// lost. The reply lists their ids in key order (`mark_lost_workers`).
pub fn mark_lost_workers(
    slice: &QueueSlice,
    stale_after_seconds: i64,
    stamp: &Stamp,
) -> Decision<Vec<String>> {
    let cutoff = shift(stamp.at, -(stale_after_seconds as f64));
    let lost: Vec<&Worker> = slice
        .workers
        .values()
        .filter(|worker| worker.status == STATUS_RUNNING && worker.heartbeat_at < cutoff)
        .collect();
    let ids: Vec<String> = lost.iter().map(|worker| worker.id.clone()).collect();
    lost.into_iter()
        .map(|worker| {
            Write::update(Worker {
                status: STATUS_LOST.to_owned(),
                ..worker.clone()
            })
        })
        .fold(Decision::new(ids), Decision::write)
}

/// The newest workers first, at most [`WORKER_LISTING_LIMIT`] (`JobQueue.workers`).
pub fn list_workers(slice: &QueueSlice) -> Vec<&Worker> {
    slice
        .workers
        .values()
        .map(|worker| ((Reverse(worker.started_at), worker.id.as_str()), worker))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .take(WORKER_LISTING_LIMIT)
        .collect()
}
