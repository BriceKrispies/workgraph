//! [`QueueSlice`]: jobs, their runs and resource claims, workers and system cursors.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{Job, JobResourceClaim, JobRun, SystemCursor, Worker};
use crate::slice::children;
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the queue family.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueSlice {
    pub jobs: RedBlackTreeMapSync<Id, Job>,
    pub runs: RedBlackTreeMapSync<Id, JobRun>,
    /// Keyed `(job_id, resource_key)`.
    pub claims: RedBlackTreeMapSync<(Id, String), JobResourceClaim>,
    /// Keyed by worker id.
    pub workers: RedBlackTreeMapSync<String, Worker>,
    /// Keyed by cursor name.
    pub cursors: RedBlackTreeMapSync<String, SystemCursor>,
}

impl QueueSlice {
    pub fn job(&self, id: Id) -> Option<&Job> {
        self.jobs.get(&id)
    }

    /// The runs of a job, by run id.
    pub fn runs_of(&self, job_id: Id) -> impl Iterator<Item = &JobRun> {
        self.runs.values().filter(move |row| row.job_id == job_id)
    }

    /// The resource keys a job claims, in key order.
    pub fn claims_of(&self, job_id: Id) -> impl Iterator<Item = &JobResourceClaim> {
        children(&self.claims, job_id)
    }

    pub fn worker(&self, id: &str) -> Option<&Worker> {
        self.workers.get(id)
    }

    /// The value of a system cursor, if it is loaded.
    pub fn cursor(&self, name: &str) -> Option<i64> {
        self.cursors.get(name).map(|cursor| cursor.value)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => self.put(row),
            Write::Delete(key) => self.remove(key),
        }
    }

    /// The slice after every write, in order.
    #[must_use]
    pub fn apply_all<'a>(&self, writes: impl IntoIterator<Item = &'a Write>) -> Self {
        writes
            .into_iter()
            .fold(self.clone(), |slice, write| slice.apply(write))
    }

    fn put(&self, row: &Row) -> Self {
        match row {
            Row::Job(row) => QueueSlice {
                jobs: self.jobs.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::JobRun(row) => QueueSlice {
                runs: self.runs.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::JobResourceClaim(row) => QueueSlice {
                claims: self
                    .claims
                    .insert((row.job_id, row.resource_key.clone()), row.clone()),
                ..self.clone()
            },
            Row::Worker(row) => QueueSlice {
                workers: self.workers.insert(row.id.clone(), row.clone()),
                ..self.clone()
            },
            Row::SystemCursor(row) => QueueSlice {
                cursors: self.cursors.insert(row.name.clone(), row.clone()),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    fn remove(&self, key: &RowKey) -> Self {
        match key {
            RowKey::Job(id) => QueueSlice {
                jobs: self.jobs.remove(id),
                ..self.clone()
            },
            RowKey::JobRun(id) => QueueSlice {
                runs: self.runs.remove(id),
                ..self.clone()
            },
            RowKey::JobResourceClaim {
                job_id,
                resource_key,
            } => QueueSlice {
                claims: self.claims.remove(&(*job_id, resource_key.clone())),
                ..self.clone()
            },
            RowKey::Worker(id) => QueueSlice {
                workers: self.workers.remove(id),
                ..self.clone()
            },
            RowKey::SystemCursor(name) => QueueSlice {
                cursors: self.cursors.remove(name),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }
}
