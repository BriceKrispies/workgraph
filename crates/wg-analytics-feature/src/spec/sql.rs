//! Source extraction for the analytics warehouse (SH-14): the `TABLES` list of
//! `analytics/warehouse.py`, the table to column subsets the warehouse copies out of the SQLite
//! database, and the statements that copy them.
//!
//! Python's extraction asks SQLite which columns a table has (`PRAGMA table_info`), keeps the
//! wanted columns that exist, in the wanted order, and selects them. This module does the same as
//! values: [`table_info`] is the probe, [`column_names`] reads its answer, [`source_columns`] is the
//! filter, and [`source_select`] is the copy. The copy has `ORDER BY rowid`, so the warehouse sees
//! the rows in the order a table scan returns them.

use wg_types::WgError;

use wg_storecodec::codec::{DbFailure, Raw, db_error};
use wg_storecodec::sql::Statement;

/// The tables the warehouse copies, each with the columns it reads (Python `TABLES`, in its
/// order). Every table is copied in this order.
pub const TABLES: &[(&str, &[&str])] = &[
    ("workspaces", &["id", "slug", "name"]),
    (
        "projects",
        &["id", "workspace_id", "slug", "name", "status"],
    ),
    (
        "repositories",
        &["id", "workspace_id", "name", "access_mode", "archived_at"],
    ),
    (
        "repository_dependencies",
        &[
            "from_repository_id",
            "to_repository_id",
            "kind",
            "description",
        ],
    ),
    ("project_repositories", &["project_id", "repository_id"]),
    (
        "tasks",
        &[
            "id",
            "workspace_id",
            "number",
            "key",
            "title",
            "status",
            "priority",
            "category",
            "parent_id",
            "assignee",
            "assigned_agent",
            "created_at",
            "updated_at",
            "started_at",
            "completed_at",
            "accepted_at",
            "archived_at",
            "auto_execute",
        ],
    ),
    ("task_projects", &["task_id", "project_id"]),
    ("task_repositories", &["task_id", "repository_id", "access"]),
    ("task_tags", &["task_id", "tag"]),
    (
        "task_dependencies",
        &["task_id", "depends_on_id", "created_at"],
    ),
    (
        "task_relations",
        &[
            "source_task_id",
            "target_task_id",
            "relation_type",
            "created_at",
        ],
    ),
    (
        "agent_attempts",
        &[
            "id",
            "workspace_id",
            "task_id",
            "number",
            "status",
            "provider",
            "adapter",
            "model",
            "parent_attempt_id",
            "retry_of_attempt_id",
            "rerun_of_attempt_id",
            "created_at",
            "queued_at",
            "started_at",
            "finished_at",
            "queue_wait_ms",
            "duration_ms",
            "exit_reason",
            "usage",
            "usage_source",
            "cost_usd",
            "cost_source",
            "modified_repository_ids",
        ],
    ),
    (
        "commits",
        &[
            "id",
            "repository_id",
            "sha",
            "author_name",
            "author_email",
            "committed_at",
            "subject",
            "is_merge",
            "patch_id",
            "insertions",
            "deletions",
            "files_changed",
            "reachable",
        ],
    ),
    (
        "commit_file_changes",
        &[
            "id",
            "commit_id",
            "path",
            "old_path",
            "change_type",
            "insertions",
            "deletions",
            "is_binary",
            "language",
            "file_class",
            "module",
        ],
    ),
    (
        "commit_task_links",
        &["commit_id", "task_id", "source", "confidence", "attempt_id"],
    ),
    (
        "commit_analyses",
        &[
            "commit_id",
            "functions_affected",
            "types_affected",
            "files_parsed",
            "files_fallback",
        ],
    ),
    (
        "commit_symbols",
        &[
            "file_change_id",
            "symbol_kind",
            "name",
            "change_kind",
            "lines_changed",
        ],
    ),
    (
        "verification_runs",
        &[
            "id",
            "workspace_id",
            "task_id",
            "attempt_id",
            "status",
            "required_passed",
            "started_at",
            "finished_at",
            "created_at",
        ],
    ),
    (
        "verification_check_results",
        &[
            "id",
            "run_id",
            "name",
            "kind",
            "required",
            "repository_id",
            "status",
            "exit_code",
            "duration_ms",
            "timed_out",
            "failure_signatures",
            "created_at",
        ],
    ),
    (
        "task_acceptances",
        &["task_id", "attempt_id", "decision", "mode", "created_at"],
    ),
    (
        "task_change_surfaces",
        &[
            "task_id",
            "repository_id",
            "files_changed",
            "insertions",
            "deletions",
            "renames",
            "binary_files",
            "functions_affected",
            "types_affected",
        ],
    ),
    ("change_sets", &["id", "task_id", "attempt_id", "status"]),
    (
        "change_set_items",
        &["change_set_id", "repository_id", "status"],
    ),
    (
        "classifications",
        &[
            "id",
            "task_id",
            "source",
            "status",
            "provider",
            "model",
            "category",
            "confidence",
            "is_current",
            "created_at",
        ],
    ),
];

/// The wanted columns of [`TABLES`] for `table`, if the table is listed there.
pub fn wanted_columns(table: &str) -> Option<&'static [&'static str]> {
    TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, columns)| *columns)
}

/// `PRAGMA table_info` for `table` (the probe Python's `_sqlite_columns` runs). Each row describes
/// one column; the name is the second value.
pub fn table_info(table: &str) -> Statement {
    Statement {
        sql: format!("PRAGMA table_info(\"{table}\")"),
        params: Vec::new(),
    }
}

/// The column names in a `table_info` answer, in table order.
pub fn column_names(rows: &[Vec<Raw>]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| match row.get(1) {
            Some(Raw::Text(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// The columns a copy reads: the wanted ones that the table has, in wanted order. Without a wanted
/// list (`None`) every column the table has is read, in table order.
pub fn source_columns(wanted: Option<&[&str]>, available: &[String]) -> Vec<String> {
    match wanted {
        Some(wanted) => wanted
            .iter()
            .filter(|column| available.iter().any(|name| name == *column))
            .map(|column| (*column).to_owned())
            .collect(),
        None => available.to_vec(),
    }
}

/// The event types the warehouse loads (`EVENT_TYPES` of `analytics/warehouse.py`).
pub const EVENT_TYPES: [&str; 6] = [
    "task.status_changed",
    "task.reopened",
    "task.completed",
    "task.accepted",
    "task.escalated",
    "task.rejected",
];

/// The columns of the `events` copy, in the order `warehouse.py` reads them.
pub const EVENT_COLUMNS: [&str; 7] = [
    "seq",
    "workspace_id",
    "event_type",
    "entity_id",
    "actor",
    "occurred_at",
    "data",
];

/// The copy of the warehouse's events: the rows of [`EVENT_TYPES`], in rowid order. Python copies
/// `events` by its own statement (not by [`TABLES`]), so this is separate from [`source_select`].
#[must_use]
pub fn events_select() -> Statement {
    let names: Vec<String> = EVENT_COLUMNS
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect();
    let placeholders = vec!["?"; EVENT_TYPES.len()].join(",");
    Statement {
        sql: format!(
            "SELECT {} FROM \"events\" WHERE \"event_type\" IN ({placeholders}) ORDER BY rowid",
            names.join(", ")
        ),
        params: EVENT_TYPES
            .iter()
            .map(|event| Raw::Text((*event).to_owned()))
            .collect(),
    }
}

/// `SELECT <columns> FROM <table> ORDER BY rowid`, with identifiers quoted. Fails with a
/// `workgraph_error` when no column is left to read, since Python's copy of such a table is a
/// syntax error.
pub fn source_select(table: &str, columns: &[String]) -> Result<Statement, WgError> {
    if columns.is_empty() {
        return Err(db_error(
            DbFailure::Other,
            &format!("no readable columns in {table} for the analytics source"),
        ));
    }
    let names: Vec<String> = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect();
    Ok(Statement {
        sql: format!(
            "SELECT {} FROM \"{table}\" ORDER BY rowid",
            names.join(", ")
        ),
        params: Vec::new(),
    })
}
