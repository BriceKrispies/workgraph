//! Queue-family reads and the atomic claim on a [`Session`] (SH-08): jobs, their runs, resource
//! claims, workers and system cursors.
//!
//! The statements come from `wg_storecodec::queue_sql`. The claim decision is `wg_jobs::queue::claim`
//! run over the rows those statements load, and its writes are applied in the same transaction, so
//! two claimers never take the same job: [`Store::claim_job`] runs the claim in `BEGIN IMMEDIATE`.

use std::collections::BTreeMap;

use wg_jobs::limits::LimitSettings;
use wg_jobs::queue::{CLAIM_CANDIDATES, ClaimedJob, JobFilter};
use wg_jobs::workers::WORKER_LISTING_LIMIT;
use wg_model::RowKey;
use wg_model::entity::{Job, JobResourceClaim, JobRun, SystemCursor, Worker};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_storecodec::{Statement, TableRow, queue_sql};
use wg_types::{Id, WgError};

use crate::{Session, Store};

impl Session<'_> {
    /// Runs a query and decodes every row through the table's codec.
    fn decode_all<T: TableRow>(&mut self, statement: &Statement) -> Result<Vec<T>, WgError> {
        self.rows(statement)?
            .iter()
            .map(|values| T::from_raw(values))
            .collect()
    }

    /// The job with `job_id`, if there is one.
    pub fn job(&mut self, job_id: Id) -> Result<Option<Job>, WgError> {
        self.fetch(&RowKey::Job(job_id))
    }

    /// A page of jobs, newest first, with the number of jobs the filter matches before paging
    /// (`JobQueue.list`).
    pub fn list_jobs(&mut self, filter: &JobFilter<'_>) -> Result<(Vec<Job>, usize), WgError> {
        let page = self.decode_all::<Job>(&queue_sql::job_page(
            filter.statuses,
            filter.kind,
            filter.workspace_id,
            filter.task_id,
            filter.offset,
            filter.limit,
        ))?;
        let count = self.rows(&queue_sql::job_count(
            filter.statuses,
            filter.kind,
            filter.workspace_id,
            filter.task_id,
        ))?;
        Ok((page, queue_sql::count_from_rows(&count)?))
    }

    /// The runs of a job, in start order (`JobQueue.runs`).
    pub fn job_runs(&mut self, job_id: Id) -> Result<Vec<JobRun>, WgError> {
        self.decode_all(&queue_sql::job_runs(job_id))
    }

    /// Job counts by status over the jobs visible in `workspace_id` (`JobQueue.stats`).
    pub fn job_stats(
        &mut self,
        workspace_id: Option<Id>,
    ) -> Result<BTreeMap<String, usize>, WgError> {
        let rows = self.rows(&queue_sql::job_stats(workspace_id))?;
        queue_sql::stats_from_rows(&rows)
    }

    /// The newest workers first, at most [`WORKER_LISTING_LIMIT`] (`JobQueue.workers`).
    pub fn workers(&mut self) -> Result<Vec<Worker>, WgError> {
        self.decode_all(&queue_sql::workers(WORKER_LISTING_LIMIT))
    }

    /// The system cursor called `name`, if it has been written.
    pub fn system_cursor(&mut self, name: &str) -> Result<Option<SystemCursor>, WgError> {
        self.fetch(&RowKey::SystemCursor(name.to_owned()))
    }

    /// The slice a claim reads: the runnable candidates for `kinds` at `stamp.at`, the running jobs
    /// that hold resource claims, and those claims.
    pub fn claim_slice(&mut self, kinds: &[&str], stamp: &Stamp) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        let candidates =
            self.decode_all::<Job>(&queue_sql::runnable_jobs(stamp.at, kinds, CLAIM_CANDIDATES))?;
        for job in candidates {
            slice.jobs = slice.jobs.insert(job.id, job);
        }
        for job in self.decode_all::<Job>(&queue_sql::running_jobs_with_claims())? {
            slice.jobs = slice.jobs.insert(job.id, job);
        }
        for claim in self.decode_all::<JobResourceClaim>(&queue_sql::running_claims())? {
            slice.claims = slice
                .claims
                .insert((claim.job_id, claim.resource_key.clone()), claim);
        }
        Ok(slice)
    }

    /// The slice an enqueue reads: the job already holding `key`, if any.
    pub fn enqueue_slice(&mut self, key: Option<&str>) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        if let Some(key) = key {
            for job in self.decode_all::<Job>(&queue_sql::job_by_idempotency_key(key))? {
                slice.jobs = slice.jobs.insert(job.id, job);
            }
        }
        Ok(slice)
    }

    /// The queued and running jobs of `kind` as a slice (what a duplicate check reads).
    pub fn active_slice(&mut self, kind: &str) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        let filter = JobFilter {
            statuses: &wg_jobs::queue::ACTIVE_STATUSES,
            kind: Some(kind),
            ..JobFilter::default()
        };
        for job in self.list_jobs(&filter)?.0 {
            slice.jobs = slice.jobs.insert(job.id, job);
        }
        Ok(slice)
    }

    /// The slice a decision on one job reads: the job, its runs and its resource claims. An
    /// unknown job gives an empty slice.
    pub fn job_slice(&mut self, job_id: Id) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        if let Some(job) = self.job(job_id)? {
            slice.jobs = slice.jobs.insert(job.id, job);
        }
        for run in self.job_runs(job_id)? {
            slice.runs = slice.runs.insert(run.id, run);
        }
        for claim in self.decode_all::<JobResourceClaim>(&queue_sql::job_claims(job_id))? {
            slice.claims = slice
                .claims
                .insert((claim.job_id, claim.resource_key.clone()), claim);
        }
        Ok(slice)
    }

    /// The slice lease recovery reads: every running job with its runs and resource claims.
    pub fn running_slice(&mut self) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        for job in self.decode_all::<Job>(&queue_sql::running_jobs())? {
            slice.jobs = slice.jobs.insert(job.id, job);
        }
        for run in self.decode_all::<JobRun>(&queue_sql::running_job_runs())? {
            slice.runs = slice.runs.insert(run.id, run);
        }
        for claim in self.decode_all::<JobResourceClaim>(&queue_sql::running_claims())? {
            slice.claims = slice
                .claims
                .insert((claim.job_id, claim.resource_key.clone()), claim);
        }
        Ok(slice)
    }

    /// The slice worker liveness reads: the worker `worker_id` (when given) and every running
    /// worker.
    pub fn worker_slice(&mut self, worker_id: Option<&str>) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        for worker in self.decode_all::<Worker>(&queue_sql::running_workers())? {
            slice.workers = slice.workers.insert(worker.id.clone(), worker);
        }
        if let Some(id) = worker_id
            && let Some(worker) = self.fetch::<Worker>(&RowKey::Worker(id.to_owned()))?
        {
            slice.workers = slice.workers.insert(worker.id.clone(), worker);
        }
        Ok(slice)
    }

    /// The slice the cadence gates read: the system cursors called `names` that exist.
    pub fn cursor_slice(&mut self, names: &[String]) -> Result<QueueSlice, WgError> {
        let mut slice = QueueSlice::default();
        for name in names {
            if let Some(cursor) = self.system_cursor(name)? {
                slice.cursors = slice.cursors.insert(cursor.name.clone(), cursor);
            }
        }
        Ok(slice)
    }

    /// Claims the next runnable job for `worker` (`JobQueue.claim`) inside this session's
    /// transaction. The candidates and the held resources are read first, the claim is decided by
    /// `wg_jobs::queue::claim`, and its writes (the job, its run, its resource claims) are applied.
    /// Call it from a write transaction (or use [`Store::claim_job`]), so the reads and the writes
    /// see one state.
    pub fn claim_job(
        &mut self,
        worker: &str,
        kinds: &[&str],
        lease_seconds: i64,
        limits: &LimitSettings,
        stamp: &Stamp,
    ) -> Result<Option<ClaimedJob>, WgError> {
        let slice = self.claim_slice(kinds, stamp)?;
        let decision = wg_jobs::queue::claim(&slice, worker, kinds, lease_seconds, limits, stamp);
        self.apply_decision(&decision)?;
        Ok(decision.reply)
    }
}

impl Store {
    /// Claims the next runnable job for `worker` in one `BEGIN IMMEDIATE` transaction. Two claims
    /// never return the same job; a claim that finds nothing runnable writes nothing.
    pub fn claim_job(
        &mut self,
        worker: &str,
        kinds: &[&str],
        lease_seconds: i64,
        limits: &LimitSettings,
        stamp: &Stamp,
    ) -> Result<Option<ClaimedJob>, WgError> {
        self.write(|session| session.claim_job(worker, kinds, lease_seconds, limits, stamp))
    }
}
