//! The `stack` family: `stacks`, `stack_repositories` and `stack_items`.
//!
//! A stack is the ordered set of child tasks of one parent task that integrate together into
//! per-repository integration branches. Its repositories fix the base each integration starts
//! from; its items carry each child's position and integration state.

use wg_types::{Id, StackItemState, StackStatus, Timestamp};

use crate::write::{Row, RowKey};

/// A row of `stacks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub id: Id,
    pub workspace_id: Id,
    pub parent_task_id: Id,
    pub status: StackStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// The land request recorded while the stack waits to land (Python `land_request`, stored
    /// as text).
    pub land_request: Option<String>,
}

/// A row of `stack_repositories`: one repository of a stack, keyed `(stack, repository)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRepository {
    pub stack_id: Id,
    pub repository_id: Id,
    pub base_sha: String,
    pub integration_branch: String,
    pub created_at: Timestamp,
}

/// A row of `stack_items`: one child task in a stack (unique per `(stack, task)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackItem {
    pub id: Id,
    pub stack_id: Id,
    pub task_id: Id,
    pub state: StackItemState,
    pub merge_order: Option<i64>,
    pub detail: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Stack {
    pub fn row_key(&self) -> RowKey {
        RowKey::Stack(self.id)
    }
}

impl StackRepository {
    pub fn row_key(&self) -> RowKey {
        RowKey::StackRepository {
            stack_id: self.stack_id,
            repository_id: self.repository_id,
        }
    }
}

impl StackItem {
    pub fn row_key(&self) -> RowKey {
        RowKey::StackItem(self.id)
    }
}

impl From<Stack> for Row {
    fn from(row: Stack) -> Self {
        Row::Stack(row)
    }
}

impl From<StackRepository> for Row {
    fn from(row: StackRepository) -> Self {
        Row::StackRepository(row)
    }
}

impl From<StackItem> for Row {
    fn from(row: StackItem) -> Self {
        Row::StackItem(row)
    }
}
