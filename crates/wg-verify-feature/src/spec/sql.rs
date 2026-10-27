//! SQL text, parameters and row decoding for the verification family (SH-10): verification
//! configs of one scope, runs, check results, and the joined failure history. The reads base code
//! shares (the workspace configs, the newest run of a task, task acceptances) are in
//! `wg_storecodec::acceptance_sql`.
//!
//! Every statement is a value ([`Statement`]); `wg-store` binds and runs it. Every SELECT lists the
//! table's [`columns`] in schema order, so its rows decode through the same `TableRow` codecs as a
//! fetch by key. The joined failure query adds the run and task columns after the check-result
//! columns; [`decode_failure`] splits them.
//!
//! Lists have a fixed `ORDER BY` ending in the row id, so ties (rows written in the same instant)
//! come back in the same order every time. The Python service leaves ties unordered.

use wg_model::Table;
use wg_model::entity::VerificationCheckResult;
use wg_storecodec::codec::{Cols, Raw, id_sql, text_sql, ts_sql};
use wg_storecodec::rows::{TableRow, columns};
use wg_storecodec::sql::{Statement, select_list};
use wg_types::{ErrorCode, Id, TaskKey, Timestamp, VerificationStatus, WgError};

/// Appends a bound parameter and returns its `?N` placeholder.
fn bind(params: &mut Vec<Raw>, value: Raw) -> String {
    params.push(value);
    format!("?{}", params.len())
}

/// `LIMIT ?n OFFSET ?m` for a page, binding both values.
fn page_clause(params: &mut Vec<Raw>, page: Page) -> String {
    let limit = bind(params, Raw::Int(page.limit));
    let offset = bind(params, Raw::Int(page.offset));
    format!(" LIMIT {limit} OFFSET {offset}")
}

/// One page of a list: at most `limit` rows, after skipping `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

/// The filters of the run list (Python's `list_runs`); `None` leaves a filter out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub status: Option<VerificationStatus>,
}

/// The filters of the failure history (Python's `failures`); `None` leaves a filter out. The
/// history only ever holds `failed` and `error` check results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FailureFilter {
    pub check_name: Option<String>,
    pub repository_id: Option<Id>,
    /// Keeps results created at or after this instant.
    pub since: Option<Timestamp>,
}

/// One failed check result with the run and task it belongs to (the joined failure query).
#[derive(Debug, Clone, PartialEq)]
pub struct FailureRow {
    pub check: VerificationCheckResult,
    pub run_id: Id,
    pub attempt_id: Option<Id>,
    pub task_id: Id,
    pub task_key: TaskKey,
}

/// The verification config of one scope and name in a workspace (the unique key of the table).
pub fn config_by_scope(workspace_id: Id, scope_type: &str, scope_id: Id, name: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"verification_configs\" WHERE \"workspace_id\" = ?1 \
             AND \"scope_type\" = ?2 AND \"scope_id\" = ?3 AND \"name\" = ?4",
            select_list(Table::VerificationConfig, None)
        ),
        params: vec![
            id_sql(workspace_id),
            text_sql(scope_type),
            id_sql(scope_id),
            text_sql(name),
        ],
    }
}

/// The runs of a workspace, newest first, narrowed by `filter` and cut to `page`.
pub fn runs_in_workspace(workspace_id: Id, filter: &RunFilter, page: Page) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    let mut sql = format!(
        "SELECT {} FROM \"verification_runs\" WHERE \"workspace_id\" = ?1",
        select_list(Table::VerificationRun, None)
    );
    if let Some(task_id) = filter.task_id {
        let n = bind(&mut params, id_sql(task_id));
        sql.push_str(&format!(" AND \"task_id\" = {n}"));
    }
    if let Some(attempt_id) = filter.attempt_id {
        let n = bind(&mut params, id_sql(attempt_id));
        sql.push_str(&format!(" AND \"attempt_id\" = {n}"));
    }
    if let Some(status) = filter.status {
        let n = bind(&mut params, text_sql(status.as_str()));
        sql.push_str(&format!(" AND \"status\" = {n}"));
    }
    sql.push_str(" ORDER BY \"created_at\" DESC, \"id\" DESC");
    sql.push_str(&page_clause(&mut params, page));
    Statement { sql, params }
}

/// The check results of a run, in the order they were recorded.
pub fn check_results_of_run(run_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"verification_check_results\" WHERE \"run_id\" = ?1 \
             ORDER BY \"created_at\", \"id\"",
            select_list(Table::VerificationCheckResult, None)
        ),
        params: vec![id_sql(run_id)],
    }
}

/// The failure history of a workspace: failed and errored check results joined to their run and
/// task, newest first, narrowed by `filter` and cut to `page`. The row is the check result's
/// columns, then the run's `id` and `attempt_id`, then the task's `id` and `key` (see
/// [`decode_failure`]).
pub fn failures(workspace_id: Id, filter: &FailureFilter, page: Page) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    let failed = bind(&mut params, text_sql(VerificationStatus::Failed.as_str()));
    let errored = bind(&mut params, text_sql(VerificationStatus::Error.as_str()));
    let mut sql = format!(
        "SELECT {}, r.\"id\", r.\"attempt_id\", t.\"id\", t.\"key\" \
         FROM \"verification_check_results\" AS c \
         JOIN \"verification_runs\" AS r ON r.\"id\" = c.\"run_id\" \
         JOIN \"tasks\" AS t ON t.\"id\" = r.\"task_id\" \
         WHERE r.\"workspace_id\" = ?1 AND c.\"status\" IN ({failed}, {errored})",
        select_list(Table::VerificationCheckResult, Some("c"))
    );
    if let Some(name) = &filter.check_name {
        let n = bind(&mut params, text_sql(name));
        sql.push_str(&format!(" AND c.\"name\" = {n}"));
    }
    if let Some(repository_id) = filter.repository_id {
        let n = bind(&mut params, id_sql(repository_id));
        sql.push_str(&format!(" AND c.\"repository_id\" = {n}"));
    }
    if let Some(since) = filter.since {
        let n = bind(&mut params, ts_sql(since));
        sql.push_str(&format!(" AND c.\"created_at\" >= {n}"));
    }
    sql.push_str(" ORDER BY c.\"created_at\" DESC, c.\"id\" DESC");
    sql.push_str(&page_clause(&mut params, page));
    Statement { sql, params }
}

/// Decodes one row of [`failures`]: the check result's columns, then the run's `id` and
/// `attempt_id`, then the task's `id` and `key`. A row of any other width is an error.
pub fn decode_failure(values: &[Raw]) -> Result<FailureRow, WgError> {
    let check_width = columns(Table::VerificationCheckResult).len();
    if values.len() != check_width + 4 {
        return Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!(
                "failure row has {} columns, expected {}",
                values.len(),
                check_width + 4
            ),
        ));
    }
    let check = VerificationCheckResult::from_raw(&values[..check_width])?;
    let run = Cols::new(&["id", "attempt_id"], &values[check_width..check_width + 2]);
    let task = Cols::new(&["id", "key"], &values[check_width + 2..]);
    Ok(FailureRow {
        check,
        run_id: run.id("id")?,
        attempt_id: run.opt_id("attempt_id")?,
        task_id: task.id("id")?,
        task_key: TaskKey::parse(&task.text("key")?)?,
    })
}

/// The repositories among `repository_ids` that have taken part in a stack
/// (`_lands_through_stacks`): at most one row, empty when none has.
pub fn stack_repository_hits(repository_ids: &[Id]) -> Statement {
    let mut params = Vec::new();
    let marks: Vec<String> = repository_ids
        .iter()
        .map(|id| bind(&mut params, id_sql(*id)))
        .collect();
    Statement {
        sql: format!(
            "SELECT \"repository_id\" FROM \"stack_repositories\" \
             WHERE \"repository_id\" IN ({}) LIMIT 1",
            marks.join(", ")
        ),
        params,
    }
}
