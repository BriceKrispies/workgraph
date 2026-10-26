//! Golden loaders shared by the GV-15 test targets: the source rows of
//! `golden/analytics/input-*.json` as an [`AnalyticsInput`], and the golden reader.

#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};
use wg_analytics::source::{
    Analysis, AnalyticsInput, AttemptRow, CheckResult, CommitLink, CommitRow, Event, FileChange,
    ProjectRow, RepositoryRow, Surface, TaskProject, TaskRelation, TaskRepository, TaskRow,
    VerificationRun,
};
use wg_types::Timestamp;

pub fn golden(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/analytics")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The rows of one source table, each as a map from column name to JSON value.
pub fn records(source: &Value, table: &str) -> Vec<Map<String, Value>> {
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

pub fn text(row: &Map<String, Value>, column: &str) -> Option<String> {
    row.get(column)?.as_str().map(str::to_owned)
}

pub fn int(row: &Map<String, Value>, column: &str) -> Option<i64> {
    row.get(column)?.as_i64()
}

pub fn real(row: &Map<String, Value>, column: &str) -> Option<f64> {
    row.get(column)?.as_f64()
}

/// Booleans are stored as 0 and 1 in the source database.
pub fn flag(row: &Map<String, Value>, column: &str) -> Option<bool> {
    match row.get(column)? {
        Value::Bool(value) => Some(*value),
        value => value.as_i64().map(|n| n != 0),
    }
}

/// A timestamp as the database stores it: an ISO text with an offset, or a naive one (UTC).
pub fn moment(row: &Map<String, Value>, column: &str) -> Option<Timestamp> {
    let value = text(row, column)?;
    Timestamp::parse_iso(&value)
        .or_else(|_| Timestamp::parse_iso(&format!("{value}+00:00")))
        .ok()
}

pub fn task(row: &Map<String, Value>) -> TaskRow {
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

pub fn attempt(row: &Map<String, Value>) -> AttemptRow {
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

pub fn commit(row: &Map<String, Value>) -> CommitRow {
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

pub fn input(source: &Value) -> AnalyticsInput {
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
                repository_id: text(row, "repository_id"),
            })
            .collect(),
        verification_runs: records(source, "verification_runs")
            .iter()
            .map(|row| VerificationRun {
                id: text(row, "id"),
                workspace_id: text(row, "workspace_id"),
                task_id: text(row, "task_id"),
                attempt_id: text(row, "attempt_id"),
                status: text(row, "status"),
                created_at: moment(row, "created_at"),
            })
            .collect(),
        check_results: records(source, "verification_check_results")
            .iter()
            .map(|row| CheckResult {
                run_id: text(row, "run_id"),
                name: text(row, "name"),
                kind: text(row, "kind"),
                repository_id: text(row, "repository_id"),
                status: text(row, "status"),
                created_at: moment(row, "created_at"),
            })
            .collect(),
        relations: records(source, "task_relations")
            .iter()
            .map(|row| TaskRelation {
                source_task_id: text(row, "source_task_id"),
                relation_type: text(row, "relation_type"),
                created_at: moment(row, "created_at"),
            })
            .collect(),
        projects: records(source, "projects")
            .iter()
            .map(|row| ProjectRow {
                id: text(row, "id"),
                workspace_id: text(row, "workspace_id"),
                slug: text(row, "slug"),
            })
            .collect(),
        repositories: records(source, "repositories")
            .iter()
            .map(|row| RepositoryRow {
                id: text(row, "id"),
                workspace_id: text(row, "workspace_id"),
                name: text(row, "name"),
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
