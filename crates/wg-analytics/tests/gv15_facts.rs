#![recursion_limit = "256"]
//! GV-15, fact tables: the tables `analytics/warehouse.py` builds from the source rows of the demo
//! and the generated database (`golden/analytics/input-*.json`) must equal the rows the Python
//! warehouse produced in DuckDB (`golden/analytics/facts-*.json`), as multisets of rows with
//! identical values.

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_analytics::attempts::AttemptFact;
use wg_analytics::commits::{TaskCode, TaskCommit, TaskFile};
use wg_analytics::source::{
    Analysis, AnalyticsInput, AttemptRow, CommitLink, CommitRow, Event, FileChange, Surface,
    TaskProject, TaskRepository, TaskRow, VerificationRun,
};
use wg_analytics::tasks::{TaskFact, TaskSurface};
use wg_analytics::{Facts, build};
use wg_types::Timestamp;

fn golden(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/analytics")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The rows of one source table, each as a map from column name to JSON value.
fn records(source: &Value, table: &str) -> Vec<Map<String, Value>> {
    let columns: Vec<&str> = source["tables"][table]["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    source["tables"][table]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .map(|row| {
            columns
                .iter()
                .zip(row)
                .map(|(column, value)| ((*column).to_string(), value.clone()))
                .collect()
        })
        .collect()
}

fn text(row: &Map<String, Value>, column: &str) -> Option<String> {
    row.get(column)?.as_str().map(str::to_owned)
}

fn int(row: &Map<String, Value>, column: &str) -> Option<i64> {
    row.get(column)?.as_i64()
}

fn real(row: &Map<String, Value>, column: &str) -> Option<f64> {
    row.get(column)?.as_f64()
}

/// Booleans are stored as 0 and 1 in the source database.
fn flag(row: &Map<String, Value>, column: &str) -> Option<bool> {
    match row.get(column)? {
        Value::Bool(value) => Some(*value),
        value => value.as_i64().map(|n| n != 0),
    }
}

/// A timestamp as the database stores it: an ISO text with an offset, or a naive one (UTC).
fn moment(row: &Map<String, Value>, column: &str) -> Option<Timestamp> {
    let value = text(row, column)?;
    Timestamp::parse_iso(&value)
        .or_else(|_| Timestamp::parse_iso(&format!("{value}+00:00")))
        .ok()
}

fn task(row: &Map<String, Value>) -> TaskRow {
    TaskRow {
        id: text(row, "id"),
        workspace_id: text(row, "workspace_id"),
        number: int(row, "number"),
        key: text(row, "key"),
        title: text(row, "title"),
        status: text(row, "status"),
        priority: text(row, "priority"),
        category: text(row, "category"),
        parent_id: text(row, "parent_id"),
        assignee: text(row, "assignee"),
        assigned_agent: text(row, "assigned_agent"),
        created_at: moment(row, "created_at"),
        updated_at: moment(row, "updated_at"),
        started_at: moment(row, "started_at"),
        completed_at: moment(row, "completed_at"),
        accepted_at: moment(row, "accepted_at"),
        archived_at: moment(row, "archived_at"),
        auto_execute: flag(row, "auto_execute"),
    }
}

fn attempt(row: &Map<String, Value>) -> AttemptRow {
    AttemptRow {
        id: text(row, "id"),
        workspace_id: text(row, "workspace_id"),
        task_id: text(row, "task_id"),
        number: int(row, "number"),
        status: text(row, "status"),
        provider: text(row, "provider"),
        adapter: text(row, "adapter"),
        model: text(row, "model"),
        parent_attempt_id: text(row, "parent_attempt_id"),
        retry_of_attempt_id: text(row, "retry_of_attempt_id"),
        rerun_of_attempt_id: text(row, "rerun_of_attempt_id"),
        created_at: moment(row, "created_at"),
        queued_at: moment(row, "queued_at"),
        started_at: moment(row, "started_at"),
        finished_at: moment(row, "finished_at"),
        queue_wait_ms: int(row, "queue_wait_ms"),
        duration_ms: int(row, "duration_ms"),
        exit_reason: text(row, "exit_reason"),
        usage: text(row, "usage"),
        usage_source: text(row, "usage_source"),
        cost_usd: real(row, "cost_usd"),
        cost_source: text(row, "cost_source"),
        modified_repository_ids: text(row, "modified_repository_ids"),
    }
}

fn commit(row: &Map<String, Value>) -> CommitRow {
    CommitRow {
        id: text(row, "id"),
        repository_id: text(row, "repository_id"),
        sha: text(row, "sha"),
        author_name: text(row, "author_name"),
        author_email: text(row, "author_email"),
        committed_at: moment(row, "committed_at"),
        subject: text(row, "subject"),
        is_merge: flag(row, "is_merge"),
        patch_id: text(row, "patch_id"),
        insertions: int(row, "insertions"),
        deletions: int(row, "deletions"),
        files_changed: int(row, "files_changed"),
        reachable: flag(row, "reachable"),
    }
}

fn input(source: &Value) -> AnalyticsInput {
    AnalyticsInput {
        tasks: records(source, "tasks").iter().map(task).collect(),
        attempts: records(source, "agent_attempts")
            .iter()
            .map(attempt)
            .collect(),
        commits: records(source, "commits").iter().map(commit).collect(),
        links: records(source, "commit_task_links")
            .iter()
            .map(|row| CommitLink {
                commit_id: text(row, "commit_id"),
                task_id: text(row, "task_id"),
                confidence: real(row, "confidence"),
            })
            .collect(),
        file_changes: records(source, "commit_file_changes")
            .iter()
            .map(|row| FileChange {
                commit_id: text(row, "commit_id"),
                path: text(row, "path"),
                insertions: int(row, "insertions"),
                deletions: int(row, "deletions"),
                module: text(row, "module"),
                file_class: text(row, "file_class"),
            })
            .collect(),
        analyses: records(source, "commit_analyses")
            .iter()
            .map(|row| Analysis {
                commit_id: text(row, "commit_id"),
                functions_affected: int(row, "functions_affected"),
                types_affected: int(row, "types_affected"),
            })
            .collect(),
        surfaces: records(source, "task_change_surfaces")
            .iter()
            .map(|row| Surface {
                task_id: text(row, "task_id"),
                files_changed: int(row, "files_changed"),
                insertions: int(row, "insertions"),
                deletions: int(row, "deletions"),
                renames: int(row, "renames"),
                binary_files: int(row, "binary_files"),
                functions_affected: int(row, "functions_affected"),
                types_affected: int(row, "types_affected"),
            })
            .collect(),
        task_projects: records(source, "task_projects")
            .iter()
            .map(|row| TaskProject {
                task_id: text(row, "task_id"),
                project_id: text(row, "project_id"),
            })
            .collect(),
        task_repositories: records(source, "task_repositories")
            .iter()
            .map(|row| TaskRepository {
                task_id: text(row, "task_id"),
            })
            .collect(),
        verification_runs: records(source, "verification_runs")
            .iter()
            .map(|row| VerificationRun {
                attempt_id: text(row, "attempt_id"),
                status: text(row, "status"),
            })
            .collect(),
        events: records(source, "events")
            .iter()
            .map(|row| Event {
                event_type: text(row, "event_type"),
                entity_id: text(row, "entity_id"),
                occurred_at: moment(row, "occurred_at"),
                data: text(row, "data"),
            })
            .collect(),
        ..Default::default()
    }
}

fn stamp(at: Option<Timestamp>) -> Option<String> {
    at.map(Timestamp::format_isoformat)
}

fn task_commit_json(row: &TaskCommit) -> Value {
    let commit = &row.commit;
    json!({
        "task_id": row.task_id,
        "id": commit.id,
        "repository_id": commit.repository_id,
        "sha": commit.sha,
        "author_name": commit.author_name,
        "author_email": commit.author_email,
        "committed_at": stamp(commit.committed_at),
        "subject": commit.subject,
        "is_merge": commit.is_merge,
        "patch_id": commit.patch_id,
        "insertions": commit.insertions,
        "deletions": commit.deletions,
        "files_changed": commit.files_changed,
        "reachable": commit.reachable,
        "confidence": row.confidence,
    })
}

fn task_code_json(row: &TaskCode) -> Value {
    json!({
        "task_id": row.task_id,
        "commits": row.commits,
        "churn_insertions": row.churn_insertions,
        "churn_deletions": row.churn_deletions,
        "repositories_touched": row.repositories_touched,
        "functions_affected": row.functions_affected,
        "types_affected": row.types_affected,
    })
}

fn task_file_json(row: &TaskFile) -> Value {
    json!({
        "task_id": row.task_id,
        "path": row.path,
        "module": row.module,
        "file_class": row.file_class,
        "repository_id": row.repository_id,
        "edits": row.edits,
        "lines": row.lines,
    })
}

fn task_surface_json(row: &TaskSurface) -> Value {
    json!({
        "task_id": row.task_id,
        "final_files": row.final_files,
        "final_insertions": row.final_insertions,
        "final_deletions": row.final_deletions,
        "final_renames": row.final_renames,
        "final_binary": row.final_binary,
        "final_functions": row.final_functions,
        "final_types": row.final_types,
        "final_repositories": row.final_repositories,
    })
}

fn task_fact_json(row: &TaskFact) -> Value {
    let task = &row.task;
    json!({
        "id": task.id,
        "workspace_id": task.workspace_id,
        "number": task.number,
        "key": task.key,
        "title": task.title,
        "status": task.status,
        "priority": task.priority,
        "category": task.category,
        "parent_id": task.parent_id,
        "assignee": task.assignee,
        "assigned_agent": task.assigned_agent,
        "created_at": stamp(task.created_at),
        "updated_at": stamp(task.updated_at),
        "started_at": stamp(task.started_at),
        "completed_at": stamp(task.completed_at),
        "accepted_at": stamp(task.accepted_at),
        "archived_at": stamp(task.archived_at),
        "auto_execute": task.auto_execute,
        "project_id": row.project_id,
        "project_count": row.project_count,
        "linked_repositories": row.linked_repositories,
        "attempts": row.attempts,
        "succeeded_attempts": row.succeeded_attempts,
        "failed_attempts": row.failed_attempts,
        "retries": row.retries,
        "execution_ms": row.execution_ms,
        "queue_wait_ms": row.queue_wait_ms,
        "first_attempt_at": stamp(row.first_attempt_at),
        "first_attempt_status": row.first_attempt_status,
        "closed_at": stamp(row.closed_at),
        "reopen_count": row.reopen_count,
        "commits": row.commits,
        "churn_insertions": row.churn_insertions,
        "churn_deletions": row.churn_deletions,
        "repositories_touched": row.repositories_touched,
        "churn_functions": row.churn_functions,
        "final_files": row.final_files,
        "final_insertions": row.final_insertions,
        "final_deletions": row.final_deletions,
        "final_functions": row.final_functions,
        "final_types": row.final_types,
        "files_touched": row.files_touched,
        "repeated_file_edits": row.repeated_file_edits,
        "elapsed_ms": row.elapsed_ms,
        "cycle_ms": row.cycle_ms,
    })
}

fn attempt_fact_json(row: &AttemptFact) -> Value {
    let attempt = &row.attempt;
    json!({
        "id": attempt.id,
        "workspace_id": attempt.workspace_id,
        "task_id": attempt.task_id,
        "number": attempt.number,
        "status": attempt.status,
        "provider": attempt.provider,
        "adapter": attempt.adapter,
        "model": attempt.model,
        "parent_attempt_id": attempt.parent_attempt_id,
        "retry_of_attempt_id": attempt.retry_of_attempt_id,
        "rerun_of_attempt_id": attempt.rerun_of_attempt_id,
        "created_at": stamp(attempt.created_at),
        "queued_at": stamp(attempt.queued_at),
        "started_at": stamp(attempt.started_at),
        "finished_at": stamp(attempt.finished_at),
        "queue_wait_ms": attempt.queue_wait_ms,
        "duration_ms": attempt.duration_ms,
        "exit_reason": attempt.exit_reason,
        "usage": attempt.usage,
        "usage_source": attempt.usage_source,
        "cost_usd": attempt.cost_usd,
        "cost_source": attempt.cost_source,
        "modified_repository_ids": attempt.modified_repository_ids,
        "task_key": row.task_key,
        "category": row.category,
        "priority": row.priority,
        "input_tokens": row.input_tokens,
        "output_tokens": row.output_tokens,
        "verification_runs": row.verification_runs,
        "verification_passed": row.verification_passed,
        "project_id": row.project_id,
    })
}

/// Every row of `expected` is matched by exactly one row of `actual` with the same JSON value.
fn assert_same_rows(table: &str, actual: &[Value], expected: &Value) {
    let expected: Vec<&Value> = expected
        .as_array()
        .map_or_else(Vec::new, |rows| rows.iter().collect());
    assert_eq!(actual.len(), expected.len(), "{table}: row count");
    let unmatched = actual.iter().fold(expected, |mut left, row| {
        match left.iter().position(|golden| *golden == row) {
            Some(index) => {
                left.remove(index);
            }
            None => panic!("{table}: no golden row equals {row}"),
        }
        left
    });
    assert!(unmatched.is_empty(), "{table}: golden rows left unmatched");
}

fn check(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("facts-{name}.json"));
    let tables = &expected["tables"];
    let Facts {
        task_commits,
        task_code,
        task_files,
        task_surface,
        task_facts,
        attempt_facts,
    } = build(&input(&source));

    assert_same_rows(
        "task_commits",
        &task_commits
            .iter()
            .map(task_commit_json)
            .collect::<Vec<_>>(),
        &tables["task_commits"],
    );
    assert_same_rows(
        "task_code",
        &task_code.iter().map(task_code_json).collect::<Vec<_>>(),
        &tables["task_code"],
    );
    assert_same_rows(
        "task_files",
        &task_files.iter().map(task_file_json).collect::<Vec<_>>(),
        &tables["task_files"],
    );
    assert_same_rows(
        "task_surface",
        &task_surface
            .iter()
            .map(task_surface_json)
            .collect::<Vec<_>>(),
        &tables["task_surface"],
    );
    assert_same_rows(
        "task_facts",
        &task_facts.iter().map(task_fact_json).collect::<Vec<_>>(),
        &tables["task_facts"],
    );
    assert_same_rows(
        "attempt_facts",
        &attempt_facts
            .iter()
            .map(attempt_fact_json)
            .collect::<Vec<_>>(),
        &tables["attempt_facts"],
    );
}

#[test]
fn gv15_demo_fact_tables_match_the_python_warehouse() {
    check("demo");
}

#[test]
fn gv15_generated_fact_tables_match_the_python_warehouse() {
    check("gen");
}

/// GV-15 reports, subagent and agent time: the golden reports of both databases. Their source
/// tables hold no subagent runs, so each report is the empty report with its notes.
fn check_reports(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let input = input(&source);
    let scope = wg_analytics::common::Scope {
        workspace_id: "ws".to_owned(),
        default_workspace_id: Some("ws".to_owned()),
    };
    let window = wg_analytics::common::Window::default();
    let reports = &expected["reports"];
    let week = wg_analytics::subagents::report(
        &input,
        &scope,
        wg_analytics::subagents::Period::Week,
        &window,
    );
    let month = wg_analytics::subagents::report(
        &input,
        &scope,
        wg_analytics::subagents::Period::Month,
        &window,
    );
    let bottlenecks = wg_analytics::agent_time::bottlenecks(&input, &scope, &window, 15);
    assert_eq!(week, reports["subagents_week"], "subagents_week ({name})");
    assert_eq!(
        month, reports["subagents_month"],
        "subagents_month ({name})"
    );
    assert_eq!(
        bottlenecks, reports["agent_time_bottlenecks"],
        "agent_time_bottlenecks ({name})"
    );
}

#[test]
fn gv15_demo_subagent_and_time_reports_match_the_python_reports() {
    check_reports("demo");
}

#[test]
fn gv15_generated_subagent_and_time_reports_match_the_python_reports() {
    check_reports("gen");
}
