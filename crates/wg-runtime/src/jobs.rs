//! The job and worker entry points of the application (IF-11): the services behind `job list`,
//! `show`, `stats`, `cancel`, `retry`, `enqueue` and `compact`, and `worker list` and `recover`.
//!
//! Each write loads the queue slice its `wg-jobs` rule reads, decides and applies inside the
//! transaction of the call. The queue's own methods ([`crate::Queue`]) delegate here, so the
//! worker and the commands run one copy of each rule.

use serde_json::{Map, Value as Json, json};
use wg_jobs::dispatch::{STALE_WORKER_SECONDS, WorkerSettings};
use wg_jobs::queue::{JobFilter, NewJob};
use wg_jobs::retention::compact_artifacts;
use wg_jobs::workers::mark_lost_workers;
use wg_json::Value;
use wg_model::{Decision, Effect, FsOp, Row, Write};
use wg_registry::boundary::is_within;
use wg_runtimespec::RequestContext;
use wg_runtimespec::jobs as view;
use wg_types::{Id, JobStatus, WgError};

use crate::app::App;
use crate::stamp::{draw, system_context};

impl App {
    /// `JobQueue.enqueue`: the id of the new job, or of the job already holding `job.idempotency_key`.
    pub fn enqueue_as(&mut self, context: &RequestContext, job: &NewJob) -> Result<Id, WgError> {
        let key = job.idempotency_key.clone();
        Ok(self
            .runtime
            .decide(
                context,
                |session| session.enqueue_slice(key.as_deref()),
                |slice, stamp| Ok(wg_jobs::queue::enqueue(slice, job, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.cancel`: the job's status afterwards.
    pub(crate) fn cancel_as(
        &mut self,
        context: &RequestContext,
        job_id: Id,
    ) -> Result<JobStatus, WgError> {
        Ok(self
            .runtime
            .decide(
                context,
                |session| session.job_slice(job_id),
                |slice, stamp| wg_jobs::queue::cancel(slice, job_id, stamp),
            )?
            .reply)
    }

    /// `JobQueue.recover_expired`: the ids of the jobs whose lease had expired.
    pub(crate) fn recover_expired_as(
        &mut self,
        context: &RequestContext,
    ) -> Result<Vec<Id>, WgError> {
        Ok(self
            .runtime
            .decide(
                context,
                |session| session.running_slice(),
                |slice, stamp| Ok(wg_jobs::queue::recover_expired(slice, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.mark_lost_workers`: the ids of the workers that stopped heartbeating.
    pub(crate) fn mark_lost_workers_as(
        &mut self,
        context: &RequestContext,
    ) -> Result<Vec<String>, WgError> {
        Ok(self
            .runtime
            .decide(
                context,
                |session| session.worker_slice(None),
                |slice, stamp| Ok(mark_lost_workers(slice, STALE_WORKER_SECONDS, stamp)),
            )?
            .reply)
    }

    /// `JobQueue.retry_dead`: gives a dead, failed or cancelled job a fresh budget and makes it
    /// runnable now.
    pub(crate) fn retry_dead_as(
        &mut self,
        context: &RequestContext,
        job_id: Id,
    ) -> Result<(), WgError> {
        self.runtime.decide(
            context,
            |session| session.job_slice(job_id),
            |slice, stamp| wg_jobs::queue::retry_dead(slice, job_id, stamp),
        )?;
        Ok(())
    }

    /// `retention.compact`: purges the telemetry artifacts past the run-log window and returns the
    /// report. A dry run reports without changing anything. Hook payload compaction waits for the
    /// knowledge-family entities, so `hook_payloads` is always 0.
    pub(crate) fn compact_telemetry_as(
        &mut self,
        context: &RequestContext,
        settings: &WorkerSettings,
        dry_run: bool,
    ) -> Result<Map<String, Json>, WgError> {
        let root = self.paths.artifacts_dir().as_str().to_owned();
        let inside = |path: &str| is_within(&format!("{root}/{path}"), &root);
        let load = |session: &mut wg_store::Session<'_>, at| {
            let (_, log_cutoff) = wg_jobs::retention::cutoffs(&settings.retention, at);
            session.purgeable_artifacts("telemetry", log_cutoff)
        };
        let report = if dry_run {
            let stamp = self.runtime.stamps.stamp(context)?;
            let artifacts = self.runtime.store.read(|session| load(session, stamp.at))?;
            compact_artifacts(&artifacts, &settings.retention, &stamp, inside).reply
        } else {
            self.runtime
                .decide(
                    context,
                    |session| load(session, draw()?.0),
                    |artifacts, stamp| {
                        let decision =
                            compact_artifacts(artifacts, &settings.retention, stamp, inside);
                        // The files go once the rows commit; removing a missing file succeeds.
                        let removals: Vec<Effect> = decision
                            .writes
                            .iter()
                            .filter_map(|write| match write {
                                Write::Update {
                                    row: Row::Artifact(artifact),
                                    ..
                                } => Some(Effect::Fs(FsOp::RemoveFile {
                                    path: format!("{root}/{}", artifact.path),
                                })),
                                _ => None,
                            })
                            .collect();
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

    /// `job enqueue KIND --payload --key`: a job created by a person (`human:cli`).
    pub fn job_enqueue(
        &mut self,
        context: &RequestContext,
        kind: &str,
        payload: &str,
        key: Option<&str>,
    ) -> Result<Value, WgError> {
        let job = NewJob {
            payload: view::enqueue_payload(payload)?,
            idempotency_key: key.map(str::to_owned),
            created_by: "human:cli".to_owned(),
            ..NewJob::new(kind)
        };
        Ok(view::enqueued_out(self.enqueue_as(context, &job)?))
    }

    /// `job list`: a page of jobs, newest first, with the number of jobs the filter matches. A
    /// status that is not a job status matches nothing; a negative limit lists every job.
    pub fn job_list(
        &mut self,
        statuses: &[String],
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Value, WgError> {
        let Some(statuses) = view::status_filter(statuses) else {
            return Ok(view::job_list_out(&[], 0));
        };
        let filter = JobFilter {
            statuses: &statuses,
            kind,
            workspace_id: None,
            task_id: None,
            offset: usize::try_from(offset).unwrap_or(0),
            limit: usize::try_from(limit).ok(),
        };
        let (jobs, total) = self
            .runtime
            .store
            .read(|session| session.list_jobs(&filter))?;
        Ok(view::job_list_out(&jobs, total))
    }

    /// `job show`: the job, its payload, result and runs.
    pub fn job_show(&mut self, job_id: &str) -> Result<Value, WgError> {
        let id = view::job_ref(job_id)?;
        let (job, runs) = self
            .runtime
            .store
            .read(|session| Ok((session.job(id)?, session.job_runs(id)?)))?;
        let job = job.ok_or_else(|| view::job_not_found(job_id))?;
        Ok(view::job_detail_out(&job, &runs))
    }

    /// `job stats`: the count of each status.
    pub fn job_stats(&mut self) -> Result<Value, WgError> {
        let stats = self.runtime.store.read(|session| session.job_stats(None))?;
        Ok(view::stats_out(&stats))
    }

    /// `job cancel`: a queued job is cancelled now, a running one is asked to stop.
    pub fn job_cancel(&mut self, context: &RequestContext, job_id: &str) -> Result<Value, WgError> {
        let id = view::job_ref(job_id)?;
        let status = self.cancel_as(context, id)?;
        Ok(view::cancelled_out(job_id, status))
    }

    /// `job retry`: re-queues a dead-lettered, failed or cancelled job.
    pub fn job_retry(&mut self, context: &RequestContext, job_id: &str) -> Result<Value, WgError> {
        let id = view::job_ref(job_id)?;
        self.retry_dead_as(context, id)?;
        Ok(view::retried_out(job_id))
    }

    /// `job compact`: the retention pass, now.
    pub fn job_compact(
        &mut self,
        context: &RequestContext,
        dry_run: bool,
    ) -> Result<Value, WgError> {
        let settings = WorkerSettings::from_config(&self.config()?);
        let report = self.compact_telemetry_as(context, &settings, dry_run)?;
        Ok(view::compact_out(&report))
    }

    /// `worker list`: the workers, newest first.
    pub fn worker_list(&mut self) -> Result<Value, WgError> {
        let workers = self.runtime.store.read(|session| session.workers())?;
        Ok(view::workers_out(&workers))
    }

    /// `worker recover`: expired job leases, lost workers and orphaned worktrees, now.
    pub fn worker_recover(&mut self, context: &RequestContext) -> Result<Value, WgError> {
        let recovered = self.recover_expired_as(context)?;
        let lost = self.mark_lost_workers_as(context)?;
        // The worktree recovery acts as the system, as `worktrees.recover(RequestContext.system("cli"))`.
        let worktrees = self.worktree_recover(&system_context("cli")?)?;
        Ok(view::recovery_out(
            &recovered,
            &lost,
            wg_runtimespec::worktrees::recovery_report_out(&worktrees),
        ))
    }
}
