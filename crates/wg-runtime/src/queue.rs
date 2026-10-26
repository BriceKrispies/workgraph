//! [`Queue`]: the durable job queue and the maintenance pass on a runtime (`JobQueue` and
//! `jobs/maintenance.py`).
//!
//! Every method is one `decide` (or a few): the store loads the slice the `wg-jobs` rule reads, the
//! rule decides, the store applies. The rules, the cadence and the plan of a maintenance pass are
//! `wg-jobs`'; this type only loads, applies and reports.

use std::path::Path;

use serde_json::{Map, Value, json};
use wg_jobs::dispatch::{STALE_WORKER_SECONDS, WorkerSettings};
use wg_jobs::limits::LimitSettings;
use wg_jobs::maintenance::{
    LEADER_LEASE_KEY, RETENTION_INTERVAL_SECONDS, Step, due, elect, plan, tracker_syncs_due,
};
use wg_jobs::queue::{ClaimedJob, NewJob};
use wg_jobs::retention::compact_artifacts;
use wg_jobs::trackers::enqueue_tracker_sync;
use wg_jobs::workers::{
    NewWorker, mark_lost_workers, register_worker, stop_worker, worker_heartbeat,
};
use wg_model::entity::JsonObject;
use wg_model::{Decision, Effect, FsOp, Row, Write};
use wg_registry::boundary::is_within;
use wg_runtimespec::RequestContext;
use wg_types::{Id, JobStatus, WgError};

use crate::app::App;
use crate::stamp::{draw, system_context};

/// The queue, over one [`App`] (one database connection).
pub struct Queue {
    pub app: App,
    context: RequestContext,
}

impl Queue {
    /// Opens the application at `home` (else the default home) for queue work.
    pub fn open(home: Option<&Path>, component: &str) -> Result<Queue, WgError> {
        Ok(Queue {
            app: App::open(home)?,
            context: system_context(component)?,
        })
    }

    /// `JobQueue.enqueue`.
    pub fn enqueue(&mut self, job: &NewJob) -> Result<Id, WgError> {
        let key = job.idempotency_key.clone();
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.enqueue_slice(key.as_deref()),
                |slice, stamp| Ok(wg_jobs::queue::enqueue(slice, job, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.cancel`: the job's status afterwards.
    pub fn cancel(&mut self, job_id: Id) -> Result<JobStatus, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.job_slice(job_id),
                |slice, stamp| wg_jobs::queue::cancel(slice, job_id, stamp),
            )?
            .reply)
    }

    /// `JobQueue.claim`: the next runnable job, claimed atomically.
    pub fn claim(
        &mut self,
        worker: &str,
        kinds: &[String],
        lease_seconds: i64,
        limits: &LimitSettings,
    ) -> Result<Option<ClaimedJob>, WgError> {
        let stamp = self.app.runtime.stamps.stamp(&self.context)?;
        let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
        self.app
            .runtime
            .store
            .claim_job(worker, &kinds, lease_seconds, limits, &stamp)
    }

    /// `JobQueue.claim_peek`: whether a job is runnable, ignoring resource limits.
    pub fn claim_peek(&mut self, kinds: &[String]) -> Result<bool, WgError> {
        let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
        let stamp = self.app.runtime.stamps.stamp(&self.context)?;
        self.app.runtime.query(
            |session| session.claim_slice(&kinds, &stamp),
            |slice| Ok(wg_jobs::queue::claim_peek(slice, &kinds, &stamp).is_some()),
        )
    }

    /// `JobQueue.heartbeat`: extends the lease; the reply says whether cancellation was requested.
    pub fn heartbeat(
        &mut self,
        job_id: Id,
        worker: &str,
        lease_seconds: i64,
    ) -> Result<bool, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.job_slice(job_id),
                |slice, stamp| {
                    wg_jobs::queue::heartbeat(slice, job_id, worker, lease_seconds, stamp)
                },
            )?
            .reply)
    }

    /// `JobQueue.complete`.
    pub fn complete(
        &mut self,
        job_id: Id,
        worker: &str,
        result: Option<JsonObject>,
    ) -> Result<(), WgError> {
        self.app.runtime.decide(
            &self.context,
            |session| session.job_slice(job_id),
            |slice, stamp| wg_jobs::queue::complete(slice, job_id, worker, result, stamp),
        )?;
        Ok(())
    }

    /// `JobQueue.fail`: the status the job moved to.
    pub fn fail(
        &mut self,
        job_id: Id,
        worker: &str,
        error: &str,
        retryable: bool,
        error_kind: &str,
    ) -> Result<JobStatus, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.job_slice(job_id),
                |slice, stamp| {
                    wg_jobs::queue::fail(
                        slice,
                        job_id,
                        worker,
                        error,
                        retryable,
                        Some(error_kind),
                        stamp,
                    )
                },
            )?
            .reply)
    }

    /// `JobQueue.mark_cancelled`.
    pub fn mark_cancelled(
        &mut self,
        job_id: Id,
        worker: &str,
        reason: &str,
    ) -> Result<(), WgError> {
        self.app.runtime.decide(
            &self.context,
            |session| session.job_slice(job_id),
            |slice, stamp| wg_jobs::queue::mark_cancelled(slice, job_id, worker, reason, stamp),
        )?;
        Ok(())
    }

    /// `JobQueue.recover_expired`: the ids of the jobs whose lease had expired.
    pub fn recover_expired(&mut self) -> Result<Vec<Id>, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.running_slice(),
                |slice, stamp| Ok(wg_jobs::queue::recover_expired(slice, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.mark_lost_workers`: the ids of the workers that stopped heartbeating.
    pub fn mark_lost_workers(&mut self) -> Result<Vec<String>, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.worker_slice(None),
                |slice, stamp| Ok(mark_lost_workers(slice, STALE_WORKER_SECONDS, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.register_worker`.
    pub fn register_worker(&mut self, worker: &NewWorker) -> Result<(), WgError> {
        self.app.runtime.decide(
            &self.context,
            |session| session.worker_slice(Some(&worker.id)),
            |slice, stamp| Ok(register_worker(slice, worker, stamp)),
        )?;
        Ok(())
    }

    /// `JobQueue.worker_heartbeat`.
    pub fn worker_heartbeat(
        &mut self,
        worker_id: &str,
        current_jobs: &[String],
        completed: i64,
        failed: i64,
    ) -> Result<(), WgError> {
        self.app.runtime.decide(
            &self.context,
            |session| session.worker_slice(Some(worker_id)),
            |slice, stamp| {
                Ok(worker_heartbeat(
                    slice,
                    worker_id,
                    current_jobs,
                    completed,
                    failed,
                    stamp,
                ))
            },
        )?;
        Ok(())
    }

    /// `JobQueue.stop_worker`.
    pub fn stop_worker(&mut self, worker_id: &str) -> Result<(), WgError> {
        self.app.runtime.decide(
            &self.context,
            |session| session.worker_slice(Some(worker_id)),
            |slice, stamp| Ok(stop_worker(slice, worker_id, stamp)),
        )?;
        Ok(())
    }

    /// `worker recover`: expired job leases and lost workers, now. Orphaned worktrees are the
    /// worktree service's and are reported as not ported.
    pub fn recover(&mut self) -> Result<Map<String, Value>, WgError> {
        let jobs = self.recover_expired()?;
        let workers = self.mark_lost_workers()?;
        Ok(json!({
            "recovered_jobs": jobs.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "lost_workers": workers,
            "worktrees": "not ported",
        })
        .as_object()
        .cloned()
        .unwrap_or_default())
    }

    /// The maintenance election (`_elect`): whether `worker_id` leads for the next `ttl` seconds.
    pub fn elect(&mut self, worker_id: &str, ttl_seconds: i64) -> Result<bool, WgError> {
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.lease_by_resource(LEADER_LEASE_KEY),
                |lease, stamp| Ok(elect(lease.as_ref(), worker_id, ttl_seconds, stamp)),
            )?
            .reply)
    }

    /// `_due`: whether the periodic step `name` is due; if so its cursor moves to now.
    pub fn due(&mut self, name: &str, interval_seconds: i64) -> Result<bool, WgError> {
        let cursor = format!("maintenance:{name}");
        Ok(self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.cursor_slice(std::slice::from_ref(&cursor)),
                |slice, stamp| Ok(due(slice, name, interval_seconds, stamp)),
            )?
            .reply)
    }

    /// `run_maintenance`: elects, then runs the plan's steps in order. The reply names the steps'
    /// results; a step whose service is not ported yet reports `"not ported"`.
    pub fn maintain(
        &mut self,
        worker_id: &str,
        settings: &WorkerSettings,
        lease_seconds: i64,
    ) -> Result<Map<String, Value>, WgError> {
        let mut report = Map::new();
        if !self.elect(worker_id, lease_seconds)? {
            report.insert("leader".to_owned(), json!(false));
            return Ok(report);
        }
        report.insert("leader".to_owned(), json!(true));
        for step in plan(settings.scheduler_enabled) {
            let (name, value) = self.step(step, settings)?;
            report.insert(name.to_owned(), value);
        }
        Ok(report)
    }

    /// One step of the plan.
    fn step(
        &mut self,
        step: Step,
        settings: &WorkerSettings,
    ) -> Result<(&'static str, Value), WgError> {
        Ok(match step {
            Step::RecoverExpired => ("recovered_jobs", json!(self.recover_expired()?.len())),
            Step::MarkLostWorkers => ("lost_workers", json!(self.mark_lost_workers()?)),
            Step::TrackerSyncs => ("tracker_syncs", self.queue_tracker_syncs(settings)?),
            Step::Retention => (
                "retention",
                if self.due("retention", RETENTION_INTERVAL_SECONDS)? {
                    Value::Object(self.compact_telemetry(settings, false)?)
                } else {
                    Value::Null
                },
            ),
            Step::ReleaseExpiredClaims => ("released_claims", json!("not ported")),
            Step::ProcessAutomationEvents => ("automation", json!("not ported")),
            Step::RunScheduledRules => ("scheduled_rules", json!("not ported")),
            Step::ScheduledRuns => ("scheduled_runs", json!("not ported")),
            Step::ReplayHooks => ("replayed_hooks", json!("not ported")),
        })
    }

    /// `_enqueue_tracker_syncs`: queues each connection's poll and full syncs once their intervals
    /// have elapsed. A connection that fails to queue is counted and the rest go on.
    fn queue_tracker_syncs(&mut self, settings: &WorkerSettings) -> Result<Value, WgError> {
        let names: Vec<String> = settings
            .trackers
            .iter()
            .flat_map(|cadence| {
                ["poll", "full"]
                    .map(|mode| format!("maintenance:tracker:{}:{mode}", cadence.connection))
            })
            .collect();
        let syncs = self
            .app
            .runtime
            .decide(
                &self.context,
                |session| session.cursor_slice(&names),
                |slice, stamp| Ok(tracker_syncs_due(slice, &settings.trackers, stamp)),
            )?
            .reply;
        let known: Vec<&str> = settings
            .trackers
            .iter()
            .map(|t| t.connection.as_str())
            .collect();
        let (mut incremental, mut full, mut failed) = (0, 0, 0);
        for sync in &syncs {
            let queued = self.app.runtime.decide(
                &self.context,
                |session| session.active_slice(wg_jobs::trackers::TRACKER_SYNC_KIND),
                |slice, stamp| {
                    enqueue_tracker_sync(
                        slice,
                        &sync.connection,
                        sync.full,
                        &known,
                        "system",
                        stamp,
                    )
                },
            );
            match (queued, sync.full) {
                (Err(error), _) => {
                    failed += 1;
                    tracing::warn!(
                        target: "workgraph.maintenance",
                        connection = %sync.connection,
                        error = %error.message,
                        "tracker sync enqueue failed"
                    );
                }
                (Ok(_), true) => full += 1,
                (Ok(_), false) => incremental += 1,
            }
        }
        Ok(json!({"incremental": incremental, "full": full, "failed": failed}))
    }

    /// `retention.compact`: purges the telemetry artifacts past the run-log window. A dry run
    /// reports without changing anything. Hook payload compaction waits for the knowledge-family
    /// entities, so `hook_payloads` is always 0.
    pub fn compact_telemetry(
        &mut self,
        settings: &WorkerSettings,
        dry_run: bool,
    ) -> Result<Map<String, Value>, WgError> {
        let root = self.app.paths.artifacts_dir().as_str().to_owned();
        let inside = |path: &str| is_within(&format!("{root}/{path}"), &root);
        let load = |session: &mut wg_store::Session<'_>, at| {
            let (_, log_cutoff) = wg_jobs::retention::cutoffs(&settings.retention, at);
            session.purgeable_artifacts("telemetry", log_cutoff)
        };
        let report = if dry_run {
            let stamp = self.app.runtime.stamps.stamp(&self.context)?;
            let artifacts = self
                .app
                .runtime
                .store
                .read(|session| load(session, stamp.at))?;
            compact_artifacts(&artifacts, &settings.retention, &stamp, inside).reply
        } else {
            self.app
                .runtime
                .decide(
                    &self.context,
                    |session| load(session, draw()?.0),
                    |artifacts, stamp| {
                        let decision =
                            compact_artifacts(artifacts, &settings.retention, stamp, inside);
                        // The files go once the rows commit; removing a missing file succeeds.
                        let removals = decision.writes.iter().filter_map(|write| match write {
                            Write::Update {
                                row: Row::Artifact(artifact),
                                ..
                            } => Some(Effect::Fs(FsOp::RemoveFile {
                                path: format!("{root}/{}", artifact.path),
                            })),
                            _ => None,
                        });
                        let removals: Vec<Effect> = removals.collect();
                        Ok(removals.into_iter().fold(decision, Decision::effect))
                    },
                )?
                .reply
        };
        Ok(json!({
            "hook_payloads": 0,
            "artifacts": report.artifacts,
            "bytes_freed": report.bytes_freed,
            "dry_run": dry_run,
        })
        .as_object()
        .cloned()
        .unwrap_or_default())
    }
}
