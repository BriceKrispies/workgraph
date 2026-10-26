//! SQL text, parameters and row decoding for structured decisions (`task_decisions`): the decisions
//! of a task, the lists the decision inbox shows (bare, or with the task each one belongs to), one
//! decision by id, and the count of a task's open decisions.
//!
//! Like [`crate::task_sql`], every statement is a value ([`Statement`]); `wg-store` binds and runs
//! it. The orderings are Python's `DecisionService` ones: open decisions by task priority rank and
//! then oldest first, with archived and cancelled tasks left out; decided ones by `decided_at`
//! newest first; the rest newest first. Ties fall back to `id` so a `LIMIT` cuts the same rows
//! every time.
//!
//! The task brief (`DecisionTaskOut`) takes two queries, as Python loads it: the decision and its
//! task in one SELECT ([`decisions_by_status_with_task`]), then the tags of those tasks
//! ([`task_tags_of`]), attached with [`attach_tags`].

use wg_model::Table;
use wg_model::entity::TaskDecision;
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, WgError};

use crate::codec::{Cols, Raw, id_sql, text_sql};
use crate::rows::{TableRow, columns};
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

/// The task columns a decision brief reads, in the order [`decision_with_task`] decodes them.
const TASK_BRIEF_COLUMNS: [&str; 5] = ["key", "title", "status", "priority", "description"];

/// A decision with the task it belongs to (`DecisionWithTaskOut`). `tags` is empty until
/// [`attach_tags`] fills it from [`task_tags_of`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionWithTask {
    pub decision: TaskDecision,
    pub task: DecisionTask,
}

/// The task brief of a decision (`DecisionTaskOut`): its key, title, status, priority,
/// description and tags (sorted by tag, as the task-tag query orders them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTask {
    pub key: TaskKey,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub description: String,
    pub tags: Vec<String>,
}

/// The `task_decisions` columns in schema order, each qualified by `alias` (`d."id"`, ...).
fn decision_columns(alias: &str) -> String {
    columns(Table::TaskDecision)
        .iter()
        .map(|name| format!("{alias}.\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The brief columns of the joined task, qualified `t.`.
fn task_brief_columns() -> String {
    TASK_BRIEF_COLUMNS
        .iter()
        .map(|name| format!("t.\"{name}\""))
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
    list(workspace_id, filter, limit, false)
}

/// [`decisions_by_status`] with each decision's task brief (see [`decode_decision_with_task`]).
pub fn decisions_by_status_with_task(
    workspace_id: Id,
    filter: DecisionFilter,
    limit: i64,
) -> Statement {
    list(workspace_id, filter, limit, true)
}

/// The shared builder of the status lists. With `brief`, the task's brief columns follow the
/// decision columns.
fn list(workspace_id: Id, filter: DecisionFilter, limit: i64, brief: bool) -> Statement {
    let mut select = decision_columns("d");
    if brief {
        select = format!("{select}, {}", task_brief_columns());
    }
    let join = " JOIN \"tasks\" AS t ON t.\"id\" = d.\"task_id\"";
    let (sql, params) = match filter {
        DecisionFilter::Open => (
            format!(
                "SELECT {select} FROM \"task_decisions\" AS d{join} \
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
                "SELECT {select} FROM \"task_decisions\" AS d{} \
                 WHERE d.\"workspace_id\" = ?1 AND d.\"status\" = ?2 \
                 ORDER BY d.\"decided_at\" DESC, d.\"id\" ASC LIMIT ?3",
                if brief { join } else { "" }
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
                    "SELECT {select} FROM \"task_decisions\" AS d{} \
                     WHERE d.\"workspace_id\" = ?1{status_clause} \
                     ORDER BY d.\"created_at\" DESC, d.\"id\" ASC LIMIT ?{limit_at}",
                    if brief { join } else { "" }
                ),
                params,
            )
        }
    };
    Statement { sql, params }
}

/// The decision with `decision_id` in `workspace_id`, if there is one.
pub fn decision_by_id(workspace_id: Id, decision_id: Id) -> Statement {
    by_id(workspace_id, decision_id, false)
}

/// [`decision_by_id`] with the task brief.
pub fn decision_by_id_with_task(workspace_id: Id, decision_id: Id) -> Statement {
    by_id(workspace_id, decision_id, true)
}

fn by_id(workspace_id: Id, decision_id: Id, brief: bool) -> Statement {
    let mut select = decision_columns("d");
    let mut from = "FROM \"task_decisions\" AS d".to_owned();
    if brief {
        select = format!("{select}, {}", task_brief_columns());
        from = format!("{from} JOIN \"tasks\" AS t ON t.\"id\" = d.\"task_id\"");
    }
    Statement {
        sql: format!("SELECT {select} {from} WHERE d.\"id\" = ?1 AND d.\"workspace_id\" = ?2"),
        params: vec![id_sql(decision_id), id_sql(workspace_id)],
    }
}

/// Decodes one row of a brief query: the decision columns, then [`TASK_BRIEF_COLUMNS`]. The
/// returned task has no tags yet.
pub fn decode_decision_with_task(values: &[Raw]) -> Result<DecisionWithTask, WgError> {
    let width = columns(Table::TaskDecision).len();
    let decision = TaskDecision::from_raw(values.get(..width).unwrap_or(values))?;
    let brief = values.get(width..).unwrap_or(&[]);
    let cols = Cols::new(&TASK_BRIEF_COLUMNS, brief);
    Ok(DecisionWithTask {
        decision,
        task: DecisionTask {
            key: TaskKey::parse(&cols.text("key")?)?,
            title: cols.text("title")?,
            status: cols.parsed("status")?,
            priority: cols.parsed("priority")?,
            description: cols.text("description")?,
            tags: Vec::new(),
        },
    })
}

/// The tags of the tasks in `task_ids`, as `(task_id, tag)` rows sorted by task then tag. With no
/// ids the statement selects nothing.
pub fn task_tags_of(task_ids: &[Id]) -> Statement {
    if task_ids.is_empty() {
        return Statement {
            sql: "SELECT \"task_id\", \"tag\" FROM \"task_tags\" WHERE 1 = 0".to_owned(),
            params: Vec::new(),
        };
    }
    let marks: Vec<String> = (1..=task_ids.len()).map(|n| format!("?{n}")).collect();
    Statement {
        sql: format!(
            "SELECT \"task_id\", \"tag\" FROM \"task_tags\" WHERE \"task_id\" IN ({}) \
             ORDER BY \"task_id\", \"tag\"",
            marks.join(", ")
        ),
        params: task_ids.iter().map(|id| id_sql(*id)).collect(),
    }
}

/// Decodes one `(task_id, tag)` row of [`task_tags_of`].
pub fn decode_task_tag(values: &[Raw]) -> Result<(Id, String), WgError> {
    let cols = Cols::new(&["task_id", "tag"], values);
    Ok((cols.id("task_id")?, cols.text("tag")?))
}

/// Fills each decision's task tags from `tags` (rows of [`decode_task_tag`]). A task with no tag
/// rows keeps an empty list.
pub fn attach_tags(rows: &mut [DecisionWithTask], tags: &[(Id, String)]) {
    for row in rows.iter_mut() {
        row.task.tags = tags
            .iter()
            .filter(|(task_id, _)| *task_id == row.decision.task_id)
            .map(|(_, tag)| tag.clone())
            .collect();
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
