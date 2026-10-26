#![recursion_limit = "256"]
//! GV-15, fact tables: the tables `analytics/warehouse.py` builds from the source rows of the demo
//! and the generated database (`golden/analytics/input-*.json`) must equal the rows the Python
//! warehouse produced in DuckDB (`golden/analytics/facts-*.json`), as multisets of rows with
//! identical values.

mod common;

use common::{golden, input};
use serde_json::{Value, json};
use wg_analytics::attempts::AttemptFact;
use wg_analytics::commits::{TaskCode, TaskCommit, TaskFile};
use wg_analytics::tasks::{TaskFact, TaskSurface};
use wg_analytics::{Facts, build};
use wg_types::Timestamp;

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
