//! The `queue` family: `jobs`, `job_runs`, `job_resource_claims`, `workers` and `system_cursors`.
//!
//! A job is a unit of background work claimed by a worker under leases; a job run is one
//! attempt of it on one worker. A system cursor is a named counter the queue advances.

use rpds::VectorSync;
use wg_types::{Id, JobStatus, Timestamp};

use crate::entity::{JsonObject, Real};
use crate::write::{Row, RowKey};

/// A row of `jobs`. Lower `priority` runs first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: Id,
    pub workspace_id: Option<Id>,
    pub kind: String,
    pub status: JobStatus,
    pub priority: i64,
    pub payload: JsonObject,
    pub result: Option<JsonObject>,
    pub run_at: Timestamp,
    pub attempts: i64,
    pub max_attempts: i64,
    pub backoff_base_seconds: Real,
    pub timeout_seconds: Option<Real>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<Timestamp>,
    pub heartbeat_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub cancel_requested: bool,
    pub idempotency_key: Option<String>,
    /// JSON list of resource keys the job needs.
    pub resource_keys: VectorSync<String>,
    pub last_error: Option<String>,
    pub error_kind: Option<String>,
    pub correlation_id: Option<Id>,
    pub causation_id: Option<Id>,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `job_runs`: one attempt of a job on one worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRun {
    pub id: Id,
    pub job_id: Id,
    pub attempt_number: i64,
    pub worker_id: String,
    /// Run outcome as the queue writes it (`running`, `succeeded`, `failed`, ...).
    pub status: String,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// A row of `job_resource_claims`: one resource a job holds, keyed `(job, resource)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobResourceClaim {
    pub job_id: Id,
    pub resource_key: String,
}

/// A row of `workers`. Keyed by the worker's id string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worker {
    pub id: String,
    pub hostname: String,
    pub pid: i64,
    /// `running`, `stopped` or `lost` as the queue writes it.
    pub status: String,
    pub concurrency: i64,
    pub kinds: VectorSync<String>,
    pub current_jobs: VectorSync<String>,
    pub jobs_completed: i64,
    pub jobs_failed: i64,
    pub started_at: Timestamp,
    pub heartbeat_at: Timestamp,
    pub stopped_at: Option<Timestamp>,
}

/// A row of `system_cursors`: a named counter. Keyed by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCursor {
    pub name: String,
    pub value: i64,
    pub updated_at: Timestamp,
}

impl Job {
    pub fn row_key(&self) -> RowKey {
        RowKey::Job(self.id)
    }
}

impl JobRun {
    pub fn row_key(&self) -> RowKey {
        RowKey::JobRun(self.id)
    }
}

impl JobResourceClaim {
    pub fn row_key(&self) -> RowKey {
        RowKey::JobResourceClaim {
            job_id: self.job_id,
            resource_key: self.resource_key.clone(),
        }
    }
}

impl Worker {
    pub fn row_key(&self) -> RowKey {
        RowKey::Worker(self.id.clone())
    }
}

impl SystemCursor {
    pub fn row_key(&self) -> RowKey {
        RowKey::SystemCursor(self.name.clone())
    }
}

impl From<Job> for Row {
    fn from(row: Job) -> Self {
        Row::Job(row)
    }
}

impl From<JobRun> for Row {
    fn from(row: JobRun) -> Self {
        Row::JobRun(row)
    }
}

impl From<JobResourceClaim> for Row {
    fn from(row: JobResourceClaim) -> Self {
        Row::JobResourceClaim(row)
    }
}

impl From<Worker> for Row {
    fn from(row: Worker) -> Self {
        Row::Worker(row)
    }
}

impl From<SystemCursor> for Row {
    fn from(row: SystemCursor) -> Self {
        Row::SystemCursor(row)
    }
}
