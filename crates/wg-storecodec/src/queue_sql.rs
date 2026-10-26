//! SQL text and parameters for the queue family (SH-08): the candidates an atomic claim reads, the
//! resource claims of running jobs, the paged job listing with its count, a job's runs, status
//! counts and the worker listing.
//!
//! Like [`crate::repo_sql`], every SELECT lists the table's [`columns`] in schema order, so its rows
//! decode through the same `TableRow` codecs as a fetch by key. The claim does not choose here:
//! the store loads the rows these statements return and `wg_jobs::queue::claim` decides, with the
//! same selection `src/workgraph/jobs/queue.py` makes (queued, due, not cancel-requested, the kinds
//! asked for, ordered by priority, run time, creation time, then id, at most one batch).

use std::collections::BTreeMap;

use wg_model::Table;
use wg_types::{ErrorCode, Id, JobStatus, Timestamp, WgError};

use crate::codec::{Raw, bool_sql, id_sql, text_sql, ts_sql};
use crate::rows::columns;
use crate::sql::Statement;

/// The table's columns, quoted and qualified by `alias` when one is given.
fn select_list(table: Table, alias: Option<&str>) -> String {
    columns(table)
        .iter()
        .map(|column| match alias {
            Some(alias) => format!("{alias}.\"{column}\""),
            None => format!("\"{column}\""),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `?first, ?first+1, ...` for `count` placeholders, or `NULL` when there are none.
fn placeholders(first: usize, count: usize) -> String {
    if count == 0 {
        return "NULL".to_owned();
    }
    (first..first + count)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A count or a limit as the SQLite integer it binds as; an out-of-range value saturates.
fn int_sql(value: usize) -> Raw {
    Raw::Int(i64::try_from(value).unwrap_or(i64::MAX))
}

/// The queued jobs a claim may take: due at `run_at`, not cancel-requested, of one of `kinds` (every
/// kind when `kinds` is empty), in claim order, at most `limit` of them.
pub fn runnable_jobs(run_at: Timestamp, kinds: &[&str], limit: usize) -> Statement {
    let mut params = vec![
        text_sql(JobStatus::Queued.as_str()),
        ts_sql(run_at),
        bool_sql(false),
    ];
    let mut sql = format!(
        "SELECT {} FROM \"jobs\" WHERE \"status\" = ?1 AND \"run_at\" <= ?2 \
         AND \"cancel_requested\" = ?3",
        select_list(Table::Job, None)
    );
    if !kinds.is_empty() {
        sql.push_str(&format!(
            " AND \"kind\" IN ({})",
            placeholders(4, kinds.len())
        ));
        params.extend(kinds.iter().map(|kind| text_sql(kind)));
    }
    let limit_at = params.len() + 1;
    params.push(int_sql(limit));
    sql.push_str(&format!(
        " ORDER BY \"priority\" ASC, \"run_at\" ASC, \"created_at\" ASC, \"id\" ASC LIMIT ?{limit_at}"
    ));
    Statement { sql, params }
}

/// The jobs that are running and hold at least one resource claim (the only running jobs a claim's
/// capacity check reads).
pub fn running_jobs_with_claims() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"jobs\" WHERE \"status\" = ?1 AND \"id\" IN \
             (SELECT \"job_id\" FROM \"job_resource_claims\") ORDER BY \"id\"",
            select_list(Table::Job, None)
        ),
        params: vec![text_sql(JobStatus::Running.as_str())],
    }
}

/// The resource claims of running jobs, as `(job_id, resource_key)` rows.
pub fn running_claims() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"job_resource_claims\" AS c JOIN \"jobs\" AS j \
             ON j.\"id\" = c.\"job_id\" WHERE j.\"status\" = ?1 \
             ORDER BY c.\"job_id\", c.\"resource_key\"",
            select_list(Table::JobResourceClaim, Some("c"))
        ),
        params: vec![text_sql(JobStatus::Running.as_str())],
    }
}

/// The `WHERE` clause of a job listing and its parameters, numbered from `?1`. Every filter that is
/// given must hold; a workspace filter also keeps jobs that belong to no workspace (`JobQueue.list`).
fn job_filter(
    statuses: &[JobStatus],
    kind: Option<&str>,
    workspace_id: Option<Id>,
    task_id: Option<Id>,
) -> (String, Vec<Raw>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    if !statuses.is_empty() {
        clauses.push(format!(
            "\"status\" IN ({})",
            placeholders(params.len() + 1, statuses.len())
        ));
        params.extend(statuses.iter().map(|status| text_sql(status.as_str())));
    }
    if let Some(kind) = kind {
        clauses.push(format!("\"kind\" = ?{}", params.len() + 1));
        params.push(text_sql(kind));
    }
    if let Some(workspace_id) = workspace_id {
        let n = params.len() + 1;
        clauses.push(format!(
            "(\"workspace_id\" = ?{n} OR \"workspace_id\" IS NULL)"
        ));
        params.push(id_sql(workspace_id));
    }
    if let Some(task_id) = task_id {
        clauses.push(format!("\"task_id\" = ?{}", params.len() + 1));
        params.push(id_sql(task_id));
    }
    let clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (clause, params)
}

/// One page of the job listing, newest first (`created_at` descending, then id). `limit` of `None`
/// lists every row after `offset`.
pub fn job_page(
    statuses: &[JobStatus],
    kind: Option<&str>,
    workspace_id: Option<Id>,
    task_id: Option<Id>,
    offset: usize,
    limit: Option<usize>,
) -> Statement {
    let (clause, mut params) = job_filter(statuses, kind, workspace_id, task_id);
    let limit_at = params.len() + 1;
    // SQLite reads a negative LIMIT as no limit.
    params.push(match limit {
        Some(limit) => int_sql(limit),
        None => Raw::Int(-1),
    });
    params.push(int_sql(offset));
    Statement {
        sql: format!(
            "SELECT {} FROM \"jobs\"{clause} ORDER BY \"created_at\" DESC, \"id\" ASC \
             LIMIT ?{limit_at} OFFSET ?{}",
            select_list(Table::Job, None),
            limit_at + 1
        ),
        params,
    }
}

/// The number of jobs the matching [`job_page`] pages over.
pub fn job_count(
    statuses: &[JobStatus],
    kind: Option<&str>,
    workspace_id: Option<Id>,
    task_id: Option<Id>,
) -> Statement {
    let (clause, params) = job_filter(statuses, kind, workspace_id, task_id);
    Statement {
        sql: format!("SELECT COUNT(*) FROM \"jobs\"{clause}"),
        params,
    }
}

/// The runs of a job, in start order (ties by attempt number, then id).
pub fn job_runs(job_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"job_runs\" WHERE \"job_id\" = ?1 \
             ORDER BY \"started_at\" ASC, \"attempt_number\" ASC, \"id\" ASC",
            select_list(Table::JobRun, None)
        ),
        params: vec![id_sql(job_id)],
    }
}

/// Every running job, by id (what lease recovery inspects).
pub fn running_jobs() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"jobs\" WHERE \"status\" = ?1 ORDER BY \"id\"",
            select_list(Table::Job, None)
        ),
        params: vec![text_sql(JobStatus::Running.as_str())],
    }
}

/// The runs of running jobs, by id.
pub fn running_job_runs() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"job_runs\" AS r JOIN \"jobs\" AS j ON j.\"id\" = r.\"job_id\"              WHERE j.\"status\" = ?1 ORDER BY r.\"id\"",
            select_list(Table::JobRun, Some("r"))
        ),
        params: vec![text_sql(JobStatus::Running.as_str())],
    }
}

/// The resource claims of one job, by resource key.
pub fn job_claims(job_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"job_resource_claims\" WHERE \"job_id\" = ?1              ORDER BY \"resource_key\"",
            select_list(Table::JobResourceClaim, None)
        ),
        params: vec![id_sql(job_id)],
    }
}

/// The job holding an idempotency key, in any status.
pub fn job_by_idempotency_key(key: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"jobs\" WHERE \"idempotency_key\" = ?1 ORDER BY \"id\"",
            select_list(Table::Job, None)
        ),
        params: vec![text_sql(key)],
    }
}

/// The workers whose status is `running`, by id.
pub fn running_workers() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"workers\" WHERE \"status\" = ?1 ORDER BY \"id\"",
            select_list(Table::Worker, None)
        ),
        params: vec![text_sql("running")],
    }
}

/// Job counts grouped by status, over the jobs visible in `workspace_id` (every job when `None`).
/// Rows are `(status, count)`; see [`stats_from_rows`].
pub fn job_stats(workspace_id: Option<Id>) -> Statement {
    let (clause, params) = job_filter(&[], None, workspace_id, None);
    Statement {
        sql: format!("SELECT \"status\", COUNT(*) FROM \"jobs\"{clause} GROUP BY \"status\""),
        params,
    }
}

/// The newest `limit` workers first (by start time, then id).
pub fn workers(limit: usize) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"workers\" ORDER BY \"started_at\" DESC, \"id\" ASC LIMIT ?1",
            select_list(Table::Worker, None)
        ),
        params: vec![int_sql(limit)],
    }
}

/// Decodes the `(status, count)` rows of [`job_stats`]. Only statuses that occur appear.
pub fn stats_from_rows(rows: &[Vec<Raw>]) -> Result<BTreeMap<String, usize>, WgError> {
    rows.iter()
        .map(|row| match row.as_slice() {
            [Raw::Text(status), Raw::Int(count)] => Ok((
                status.clone(),
                usize::try_from(*count).unwrap_or(usize::MAX),
            )),
            _ => Err(shape_error("job_stats")),
        })
        .collect()
}

/// Decodes the single `COUNT(*)` value of a count statement.
pub fn count_from_rows(rows: &[Vec<Raw>]) -> Result<usize, WgError> {
    match rows.first().map(Vec::as_slice) {
        Some([Raw::Int(count)]) => Ok(usize::try_from(*count).unwrap_or(usize::MAX)),
        _ => Err(shape_error("count")),
    }
}

fn shape_error(what: &str) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("unexpected row shape from {what}"),
    )
}
