//! SQL text and parameters for structured decisions (`task_decisions`): the decisions of a task,
//! the list the decision inbox shows, one decision by id, and the count of a task's open decisions.
//!
//! Like [`crate::task_sql`], every statement is a value ([`Statement`]); `wg-store` binds and runs
//! it. The orderings are Python's `DecisionService` ones: open decisions by task priority rank and
//! then oldest first, with archived and cancelled tasks left out; decided ones by `decided_at`
//! newest first; the rest newest first. Ties fall back to `id` so a `LIMIT` cuts the same rows
//! every time.

use wg_model::Table;
use wg_types::{Id, TaskPriority, TaskStatus};

use crate::codec::{Raw, id_sql, text_sql};
use crate::rows::columns;
use crate::sql::Statement;

/// The status a decision list selects (Python's `DecisionFilter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionFilter {
    Open,
    Decided,
    Withdrawn,
    All,
}

impl DecisionFilter {
    /// The stored `status` the filter selects; `None` for every status.
    pub const fn status(self) -> Option<&'static str> {
        match self {
            DecisionFilter::Open => Some("open"),
            DecisionFilter::Decided => Some("decided"),
            DecisionFilter::Withdrawn => Some("withdrawn"),
            DecisionFilter::All => None,
        }
    }
}

/// The `task_decisions` columns in schema order, each qualified by `alias` (`d."id"`, ...).
fn decision_columns(alias: &str) -> String {
    columns(Table::TaskDecision)
        .iter()
        .map(|name| format!("{alias}.\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ORDER BY` expression for open decisions: the task's priority rank (`critical` first, unknown
/// priorities last), then the decision's age, oldest first.
fn open_order() -> String {
    let rank = [
        TaskPriority::Critical,
        TaskPriority::High,
        TaskPriority::Medium,
        TaskPriority::Low,
    ]
    .iter()
    .enumerate()
    .map(|(rank, priority)| format!("WHEN '{}' THEN {rank}", priority.as_str()))
    .collect::<Vec<_>>()
    .join(" ");
    format!("CASE t.\"priority\" {rank} ELSE 9 END ASC, d.\"created_at\" ASC, d.\"id\" ASC")
}

/// The decisions of one task, oldest first.
pub fn decisions_for_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_decisions\" AS d WHERE d.\"task_id\" = ?1 \
             ORDER BY d.\"created_at\" ASC, d.\"id\" ASC",
            decision_columns("d")
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The decisions of a workspace with the given status, at most `limit` of them, in the order
/// [`DecisionFilter`] names. Open decisions join their task so archived and cancelled tasks drop
/// out and the priority rank can be read.
pub fn decisions_by_status(workspace_id: Id, filter: DecisionFilter, limit: i64) -> Statement {
    let columns = decision_columns("d");
    let (sql, params) = match filter {
        DecisionFilter::Open => (
            format!(
                "SELECT {columns} FROM \"task_decisions\" AS d \
                 JOIN \"tasks\" AS t ON t.\"id\" = d.\"task_id\" \
                 WHERE d.\"workspace_id\" = ?1 AND d.\"status\" = ?2 \
                 AND t.\"archived_at\" IS NULL AND t.\"status\" <> ?3 \
                 ORDER BY {} LIMIT ?4",
                open_order()
            ),
            vec![
                id_sql(workspace_id),
                text_sql("open"),
                text_sql(TaskStatus::Cancelled.as_str()),
                Raw::Int(limit),
            ],
        ),
        DecisionFilter::Decided => (
            format!(
                "SELECT {columns} FROM \"task_decisions\" AS d \
                 WHERE d.\"workspace_id\" = ?1 AND d.\"status\" = ?2 \
                 ORDER BY d.\"decided_at\" DESC, d.\"id\" ASC LIMIT ?3"
            ),
            vec![id_sql(workspace_id), text_sql("decided"), Raw::Int(limit)],
        ),
        DecisionFilter::Withdrawn | DecisionFilter::All => {
            let status_clause = match filter.status() {
                Some(_) => " AND d.\"status\" = ?2",
                None => "",
            };
            let mut params = vec![id_sql(workspace_id)];
            if let Some(status) = filter.status() {
                params.push(text_sql(status));
            }
            let limit_at = params.len() + 1;
            params.push(Raw::Int(limit));
            (
                format!(
                    "SELECT {columns} FROM \"task_decisions\" AS d \
                     WHERE d.\"workspace_id\" = ?1{status_clause} \
                     ORDER BY d.\"created_at\" DESC, d.\"id\" ASC LIMIT ?{limit_at}"
                ),
                params,
            )
        }
    };
    Statement { sql, params }
}

/// The decision with `decision_id` in `workspace_id`, if there is one.
pub fn decision_by_id(workspace_id: Id, decision_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_decisions\" AS d WHERE d.\"id\" = ?1 AND d.\"workspace_id\" = ?2",
            decision_columns("d")
        ),
        params: vec![id_sql(decision_id), id_sql(workspace_id)],
    }
}

/// How many of a task's decisions are open: a single integer row.
pub fn open_decision_count(task_id: Id) -> Statement {
    Statement {
        sql: "SELECT COUNT(*) FROM \"task_decisions\" WHERE \"task_id\" = ?1 AND \"status\" = ?2"
            .to_owned(),
        params: vec![id_sql(task_id), text_sql("open")],
    }
}
