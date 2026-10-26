//! [`RunSlice`]: attempts and the rows that belong to them, and [`RunState`]: the state an
//! attempt's run flow carries between steps, read from its `agent_attempts` row.

use rpds::{RedBlackTreeMapSync, VectorSync};
use wg_types::{AttemptStatus, Id};

use crate::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, ManagedWorktree, ResourceLease,
    RevisionManifest,
};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the run family.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunSlice {
    pub attempts: RedBlackTreeMapSync<Id, AgentAttempt>,
    pub manifests: RedBlackTreeMapSync<Id, RevisionManifest>,
    pub worktrees: RedBlackTreeMapSync<Id, ManagedWorktree>,
    /// Keyed by resource key.
    pub leases: RedBlackTreeMapSync<String, ResourceLease>,
    pub change_sets: RedBlackTreeMapSync<Id, ChangeSet>,
    pub change_set_items: RedBlackTreeMapSync<Id, ChangeSetItem>,
    pub artifacts: RedBlackTreeMapSync<Id, Artifact>,
}

impl RunSlice {
    pub fn attempt(&self, id: Id) -> Option<&AgentAttempt> {
        self.attempts.get(&id)
    }

    /// The attempts of a task, by attempt id.
    pub fn attempts_of(&self, task_id: Id) -> impl Iterator<Item = &AgentAttempt> {
        self.attempts
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    pub fn manifest(&self, id: Id) -> Option<&RevisionManifest> {
        self.manifests.get(&id)
    }

    pub fn worktree(&self, id: Id) -> Option<&ManagedWorktree> {
        self.worktrees.get(&id)
    }

    pub fn lease(&self, resource_key: &str) -> Option<&ResourceLease> {
        self.leases.get(resource_key)
    }

    pub fn change_set(&self, id: Id) -> Option<&ChangeSet> {
        self.change_sets.get(&id)
    }

    /// The items of a change set, by item id.
    pub fn change_set_items_of(&self, change_set_id: Id) -> impl Iterator<Item = &ChangeSetItem> {
        self.change_set_items
            .values()
            .filter(move |row| row.change_set_id == change_set_id)
    }

    /// The flow state of an attempt, if its row is loaded.
    pub fn run_state(&self, attempt_id: Id) -> Option<RunState> {
        self.attempt(attempt_id).map(RunState::of)
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
            Row::AgentAttempt(row) => RunSlice {
                attempts: self.attempts.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::RevisionManifest(row) => RunSlice {
                manifests: self.manifests.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::ManagedWorktree(row) => RunSlice {
                worktrees: self.worktrees.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::ResourceLease(row) => RunSlice {
                leases: self.leases.insert(row.resource_key.clone(), row.clone()),
                ..self.clone()
            },
            Row::ChangeSet(row) => RunSlice {
                change_sets: self.change_sets.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::ChangeSetItem(row) => RunSlice {
                change_set_items: self.change_set_items.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::Artifact(row) => RunSlice {
                artifacts: self.artifacts.insert(row.id, row.clone()),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    fn remove(&self, key: &RowKey) -> Self {
        match key {
            RowKey::AgentAttempt(id) => RunSlice {
                attempts: self.attempts.remove(id),
                ..self.clone()
            },
            RowKey::RevisionManifest(id) => RunSlice {
                manifests: self.manifests.remove(id),
                ..self.clone()
            },
            RowKey::ManagedWorktree(id) => RunSlice {
                worktrees: self.worktrees.remove(id),
                ..self.clone()
            },
            RowKey::ResourceLease(resource_key) => RunSlice {
                leases: self.leases.remove(resource_key),
                ..self.clone()
            },
            RowKey::ChangeSet(id) => RunSlice {
                change_sets: self.change_sets.remove(id),
                ..self.clone()
            },
            RowKey::ChangeSetItem(id) => RunSlice {
                change_set_items: self.change_set_items.remove(id),
                ..self.clone()
            },
            RowKey::Artifact(id) => RunSlice {
                artifacts: self.artifacts.remove(id),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }
}

/// The state of one attempt's run flow: the fields the flow reads from its `agent_attempts` row
/// before each step. The flow tasks (`wg-exec`) extend this as they need more of the row; the
/// status is the only field the transitions themselves depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    pub attempt_id: Id,
    pub task_id: Id,
    pub status: AttemptStatus,
    pub cancel_requested: bool,
    pub retries: i64,
    /// The repositories the attempt has modified so far (JSON list of ids in the row).
    pub modified_repository_ids: VectorSync<String>,
}

impl RunState {
    /// The flow state of an attempt row.
    pub fn of(attempt: &AgentAttempt) -> Self {
        RunState {
            attempt_id: attempt.id,
            task_id: attempt.task_id,
            status: attempt.status,
            cancel_requested: attempt.cancel_requested,
            retries: attempt.retries,
            modified_repository_ids: attempt.modified_repository_ids.clone(),
        }
    }

    /// Whether the attempt has reached a terminal status (no further run step applies).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            AttemptStatus::Succeeded
                | AttemptStatus::Failed
                | AttemptStatus::Cancelled
                | AttemptStatus::TimedOut
                | AttemptStatus::Abandoned
        )
    }
}
