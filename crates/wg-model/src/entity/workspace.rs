//! The `workspace` family: the `workspaces` table.

use wg_types::{Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `workspaces`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: Id,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub key_prefix: String,
    pub next_task_number: i64,
    pub settings: JsonObject,
    pub archived_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Workspace {
    pub fn row_key(&self) -> RowKey {
        RowKey::Workspace(self.id)
    }
}

impl From<Workspace> for Row {
    fn from(row: Workspace) -> Self {
        Row::Workspace(row)
    }
}
