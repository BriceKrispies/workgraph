//! The pure part of `export analytics` (`cli/commands/data.py`, `AnalyticsService.export`): the
//! fact tables as datasets, in the column order the warehouse's `SELECT *` gives them.
//!
//! Each table is a list of column names, then one row of [`Raw`] values per fact. The names are
//! the SQL aliases of `analytics/warehouse.py` `_build`, and the source columns come first in the
//! order `TABLES` copies them (the `Task`, `AgentAttempt` and `Commit` rows of `wg-analytics`).
//! DuckDB types the booleans and timestamps; a boolean column is marked in the dataset, and a
//! timestamp is written as DuckDB writes it (`YYYY-MM-DD HH:MM:SS[.ffffff]`, fraction trimmed).

use wg_analytics::Facts;
use wg_analytics::source::{AnalyticsInput, AttemptRow, CommitRow, TaskRow};
use wg_analytics::tasks::TaskFact;
use wg_archivespec::Dataset;
use wg_rows::Raw;
use wg_types::Timestamp;

/// The fact tables `export analytics --dataset` accepts (`FACT_TABLES`), sorted as Python lists them.
pub const FACT_TABLES: [&str; 4] = ["attempt_facts", "task_commits", "task_facts", "task_files"];

/// The message of a dataset that is not a fact table: Python's `ValidationFailed` text.
pub const BAD_DATASET: &str = "dataset must be one of ['attempt_facts', 'task_commits', 'task_facts', 'task_files'] or a metric query";

const TASK_COLUMNS: [&str; 18] = [
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
];

const TASK_FACT_COLUMNS: [&str; 27] = [
    "project_id",
    "project_count",
    "linked_repositories",
    "attempts",
    "succeeded_attempts",
    "failed_attempts",
    "retries",
    "execution_ms",
    "queue_wait_ms",
    "first_attempt_at",
    "first_attempt_status",
    "closed_at",
    "reopen_count",
    "commits",
    "churn_insertions",
    "churn_deletions",
    "repositories_touched",
    "churn_functions",
    "final_files",
    "final_insertions",
    "final_deletions",
    "final_functions",
    "final_types",
    "files_touched",
    "repeated_file_edits",
    "elapsed_ms",
    "cycle_ms",
];

const ATTEMPT_COLUMNS: [&str; 23] = [
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
];

const ATTEMPT_FACT_EXTRA: [&str; 8] = [
    "task_key",
    "category",
    "priority",
    "input_tokens",
    "output_tokens",
    "verification_runs",
    "verification_passed",
    "project_id",
];

const COMMIT_COLUMNS: [&str; 13] = [
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
];

const FILE_COLUMNS: [&str; 7] = [
    "task_id",
    "path",
    "module",
    "file_class",
    "repository_id",
    "edits",
    "lines",
];

/// Python's `str(dt)` in DuckDB's form: the fraction is dropped when zero and its trailing zeros
/// are trimmed otherwise.
#[must_use]
pub fn duckdb_timestamp(at: Timestamp) -> String {
    let value = at.to_offset_datetime();
    let mut text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    );
    let micros = value.microsecond();
    if micros != 0 {
        let fraction = format!("{micros:06}");
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    text
}

fn text(value: &Option<String>) -> Raw {
    value.clone().map_or(Raw::Null, Raw::Text)
}

fn int(value: i64) -> Raw {
    Raw::Int(value)
}

fn opt_int(value: Option<i64>) -> Raw {
    value.map_or(Raw::Null, Raw::Int)
}

fn real(value: Option<f64>) -> Raw {
    value.map_or(Raw::Null, Raw::Real)
}

fn flag(value: Option<bool>) -> Raw {
    value.map_or(Raw::Null, |on| Raw::Int(i64::from(on)))
}

fn when(value: Option<Timestamp>) -> Raw {
    value.map_or(Raw::Null, |at| Raw::Text(duckdb_timestamp(at)))
}

fn task_cells(task: &TaskRow) -> Vec<Raw> {
    vec![
        text(&task.id),
        text(&task.workspace_id),
        opt_int(task.number),
        text(&task.key),
        text(&task.title),
        text(&task.status),
        text(&task.priority),
        text(&task.category),
        text(&task.parent_id),
        text(&task.assignee),
        text(&task.assigned_agent),
        when(task.created_at),
        when(task.updated_at),
        when(task.started_at),
        when(task.completed_at),
        when(task.accepted_at),
        when(task.archived_at),
        flag(task.auto_execute),
    ]
}

fn attempt_cells(attempt: &AttemptRow) -> Vec<Raw> {
    vec![
        text(&attempt.id),
        text(&attempt.workspace_id),
        text(&attempt.task_id),
        opt_int(attempt.number),
        text(&attempt.status),
        text(&attempt.provider),
        text(&attempt.adapter),
        text(&attempt.model),
        text(&attempt.parent_attempt_id),
        text(&attempt.retry_of_attempt_id),
        text(&attempt.rerun_of_attempt_id),
        when(attempt.created_at),
        when(attempt.queued_at),
        when(attempt.started_at),
        when(attempt.finished_at),
        opt_int(attempt.queue_wait_ms),
        opt_int(attempt.duration_ms),
        text(&attempt.exit_reason),
        text(&attempt.usage),
        text(&attempt.usage_source),
        real(attempt.cost_usd),
        text(&attempt.cost_source),
        text(&attempt.modified_repository_ids),
    ]
}

fn commit_cells(commit: &CommitRow) -> Vec<Raw> {
    vec![
        text(&commit.id),
        text(&commit.repository_id),
        text(&commit.sha),
        text(&commit.author_name),
        text(&commit.author_email),
        when(commit.committed_at),
        text(&commit.subject),
        flag(commit.is_merge),
        text(&commit.patch_id),
        opt_int(commit.insertions),
        opt_int(commit.deletions),
        opt_int(commit.files_changed),
        flag(commit.reachable),
    ]
}

fn names(parts: &[&[&str]]) -> Vec<String> {
    parts
        .iter()
        .flat_map(|part| part.iter())
        .map(|name| (*name).to_owned())
        .collect()
}

/// The task ids that belong to `workspace`: the rows `JOIN tasks ... WHERE workspace_id = ?` keep.
fn workspace_tasks(input: &AnalyticsInput, workspace: &str) -> Vec<String> {
    input
        .tasks
        .iter()
        .filter(|task| task.workspace_id.as_deref() == Some(workspace))
        .filter_map(|task| task.id.clone())
        .collect()
}

/// The `dataset` of `export analytics` for `workspace`: one of [`FACT_TABLES`], built from the
/// fact tables in `facts` and the source rows in `input`. The caller has checked the name.
#[must_use]
pub fn fact_dataset(
    facts: &Facts,
    input: &AnalyticsInput,
    dataset: &str,
    workspace: &str,
) -> Dataset {
    match dataset {
        "task_facts" => {
            let rows = facts
                .task_facts
                .iter()
                .filter(|fact| fact.task.workspace_id.as_deref() == Some(workspace))
                .map(task_fact_row)
                .collect();
            Dataset {
                columns: names(&[&TASK_COLUMNS[..], &TASK_FACT_COLUMNS[..]]),
                rows,
                boolean_columns: vec![TASK_COLUMNS.len() - 1],
            }
        }
        "attempt_facts" => {
            let rows = facts
                .attempt_facts
                .iter()
                .filter(|fact| fact.attempt.workspace_id.as_deref() == Some(workspace))
                .map(|fact| {
                    let mut row = attempt_cells(&fact.attempt);
                    row.extend([
                        text(&fact.task_key),
                        text(&fact.category),
                        text(&fact.priority),
                        opt_int(fact.input_tokens),
                        opt_int(fact.output_tokens),
                        int(fact.verification_runs),
                        int(fact.verification_passed),
                        text(&fact.project_id),
                    ]);
                    row
                })
                .collect();
            Dataset {
                columns: names(&[&ATTEMPT_COLUMNS[..], &ATTEMPT_FACT_EXTRA[..]]),
                rows,
                boolean_columns: Vec::new(),
            }
        }
        "task_commits" => {
            let tasks = workspace_tasks(input, workspace);
            let rows = facts
                .task_commits
                .iter()
                .filter(|link| link.task_id.as_ref().is_some_and(|id| tasks.contains(id)))
                .map(|link| {
                    let mut row = vec![text(&link.task_id)];
                    row.extend(commit_cells(&link.commit));
                    row.push(real(link.confidence));
                    row
                })
                .collect();
            let mut columns = vec!["task_id".to_owned()];
            columns.extend(names(&[&COMMIT_COLUMNS[..]]));
            columns.push("confidence".to_owned());
            Dataset {
                columns,
                rows,
                // `task_id` comes first, so is_merge and reachable sit one column later.
                boolean_columns: vec![1 + 7, 1 + 12],
            }
        }
        _ => {
            let tasks = workspace_tasks(input, workspace);
            let rows = facts
                .task_files
                .iter()
                .filter(|file| file.task_id.as_ref().is_some_and(|id| tasks.contains(id)))
                .map(|file| {
                    vec![
                        text(&file.task_id),
                        text(&file.path),
                        text(&file.module),
                        text(&file.file_class),
                        text(&file.repository_id),
                        int(file.edits),
                        opt_int(file.lines),
                    ]
                })
                .collect();
            Dataset {
                columns: names(&[&FILE_COLUMNS[..]]),
                rows,
                boolean_columns: Vec::new(),
            }
        }
    }
}

fn task_fact_row(fact: &TaskFact) -> Vec<Raw> {
    let mut row = task_cells(&fact.task);
    row.extend([
        text(&fact.project_id),
        int(fact.project_count),
        int(fact.linked_repositories),
        int(fact.attempts),
        int(fact.succeeded_attempts),
        int(fact.failed_attempts),
        int(fact.retries),
        opt_int(fact.execution_ms),
        opt_int(fact.queue_wait_ms),
        when(fact.first_attempt_at),
        text(&fact.first_attempt_status),
        when(fact.closed_at),
        int(fact.reopen_count),
        int(fact.commits),
        int(fact.churn_insertions),
        int(fact.churn_deletions),
        int(fact.repositories_touched),
        int(fact.churn_functions),
        opt_int(fact.final_files),
        opt_int(fact.final_insertions),
        opt_int(fact.final_deletions),
        opt_int(fact.final_functions),
        opt_int(fact.final_types),
        int(fact.files_touched),
        int(fact.repeated_file_edits),
        opt_int(fact.elapsed_ms),
        opt_int(fact.cycle_ms),
    ]);
    row
}

/// The `analytics export` result document (`AnalyticsService.export`): path, format, dataset and bytes.
#[must_use]
pub fn export_document(path: &str, format: &str, dataset: &str, bytes: u64) -> wg_json::Value {
    use wg_json::{Value, dict};
    use wg_wire::convention::text;
    dict!({
        "path": text(path),
        "format": text(format),
        "dataset": text(dataset),
        "bytes": Value::Int(i128::from(bytes)),
    })
}

/// The human line of `analytics export`: `wrote <path> (<bytes> bytes)`.
#[must_use]
pub fn export_view(console: &wg_render::Console, path: &str, bytes: u64) -> String {
    console
        .print(&format!(
            "wrote {} ({bytes} bytes)",
            wg_render::escape(path)
        ))
        .text
}
