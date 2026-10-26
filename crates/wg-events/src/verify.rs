//! `events verify` (C067, `core/services/history.py` `verify_task_projection`): replay the task
//! events, then compare each task's replayed state with its current-state row. A task whose row
//! differs in any compared field, or that has no `task.created` event, is a mismatch.

use serde_json::{Value, json};
use wg_model::{JsonObject, StoredEvent};
use wg_types::Id;

use crate::projection::{ProjectedTask, ProjectionError, project_tasks};
use crate::upcast::read;

/// A task's current-state row, as `events verify` reads it from the tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskState {
    pub id: Id,
    pub key: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub tags: Vec<String>,
    pub project_ids: Vec<String>,
    pub repository_ids: Vec<String>,
    pub parent_id: Option<String>,
    pub depends_on: Vec<String>,
    pub archived: bool,
}

impl TaskState {
    /// The row in the projection's shape, so both sides compare through one `comparable()`.
    #[must_use]
    pub fn as_projected(&self) -> ProjectedTask {
        ProjectedTask {
            id: self.id,
            key: json!(self.key),
            title: json!(self.title),
            description: json!(self.description),
            status: json!(self.status),
            priority: json!(self.priority),
            tags: self.tags.clone(),
            project_ids: self.project_ids.clone(),
            repository_ids: self.repository_ids.clone(),
            parent_id: json!(self.parent_id),
            depends_on: self.depends_on.clone(),
            archived: self.archived,
        }
    }
}

/// Why one task does not match its replay.
#[derive(Debug, Clone, PartialEq)]
pub enum Mismatch {
    /// The task has no `task.created` event.
    NoCreated,
    /// The compared fields that differ: the field, the table's value and the replay's value.
    Fields(Vec<(String, Value, Value)>),
}

impl Mismatch {
    fn to_json(&self) -> Value {
        match self {
            Self::NoCreated => json!({"error": "no task.created event"}),
            Self::Fields(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(name, table, events)| {
                        (name.clone(), json!({"table": table, "events": events}))
                    })
                    .collect::<JsonObject>(),
            ),
        }
    }
}

/// The result of `events verify` for one workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyReport {
    /// The tasks in the current-state tables.
    pub tasks: usize,
    /// The task events replayed.
    pub events: usize,
    /// Mismatches by task key, in the order of the tables.
    pub mismatches: Vec<(String, Mismatch)>,
}

impl VerifyReport {
    #[must_use]
    pub fn consistent(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// The report in the shape `events verify` prints (`history.py` `verify_task_projection`).
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mismatches: JsonObject = self
            .mismatches
            .iter()
            .map(|(key, mismatch)| (key.clone(), mismatch.to_json()))
            .collect();
        json!({
            "tasks": self.tasks,
            "events": self.events,
            "consistent": self.consistent(),
            "mismatches": mismatches,
        })
    }
}

/// The fields whose table value differs from the replayed one, in the order of the table side.
fn field_diffs(table: &JsonObject, replayed: &JsonObject) -> Vec<(String, Value, Value)> {
    table
        .iter()
        .filter_map(|(name, table_value)| {
            let replayed_value = replayed.get(name).cloned().unwrap_or(Value::Null);
            (*table_value != replayed_value)
                .then(|| (name.clone(), table_value.clone(), replayed_value))
        })
        .collect()
}

/// `verify_task_projection`: the events are the workspace's task events (any order, upcast on
/// read here); the tables are its task rows.
///
/// # Errors
///
/// Returns a [`ProjectionError`] when an event payload cannot be replayed.
pub fn verify_task_projection(
    events: Vec<StoredEvent>,
    tables: &[TaskState],
) -> Result<VerifyReport, ProjectionError> {
    let count = events.len();
    let projected = project_tasks(events.into_iter().map(read).collect())?;
    let mismatches = tables
        .iter()
        .filter_map(|table| {
            let row = table.as_projected();
            match projected.get(&table.id) {
                None => Some((table.key.clone(), Mismatch::NoCreated)),
                Some(replayed) => {
                    let fields = field_diffs(&row.comparable(), &replayed.comparable());
                    (!fields.is_empty()).then(|| (table.key.clone(), Mismatch::Fields(fields)))
                }
            }
        })
        .collect();
    Ok(VerifyReport {
        tasks: tables.len(),
        events: count,
        mismatches,
    })
}
