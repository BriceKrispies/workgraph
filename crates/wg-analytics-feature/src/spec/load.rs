//! Pure: the analytics source copies (`AnalyticsSource`) as the typed source rows of
//! `wg-analytics` (`AnalyticsInput`).
//!
//! Python's `_load` casts every copied column in DuckDB: timestamps by `TRY_CAST` of the text
//! without its `+00:00` suffix (so a value that does not read is NULL), booleans by `CAST`, and
//! the rest by their own type. The readers below do the same on [`Raw`] cells. A table that was
//! not copied, or a column it does not have, reads as NULL, as the copy's missing columns do.

use rpds::VectorSync;
use wg_analytics::source::{
    AcceptanceRow, Analysis, AnalyticsInput, AttemptRow, CheckResult, CommitLink, CommitRow, Event,
    FileChange, ProjectRow, RepositoryDependency, RepositoryRow, SubagentRunRow, Surface,
    TaskDependency, TaskProject, TaskRelation, TaskRepository, TaskRow, VerificationRun,
};
use wg_rows::Raw;
use wg_types::Timestamp;

use crate::spec::dto::AnalyticsSource;

/// One copied row: the column names of its table and the cells, in the same order.
#[derive(Clone, Copy)]
struct Record<'a> {
    columns: &'a [String],
    values: &'a [Raw],
}

impl<'a> Record<'a> {
    /// The cell of `column`, `None` when the table has no such column.
    fn cell(&self, column: &str) -> Option<&'a Raw> {
        let index = self.columns.iter().position(|name| name == column)?;
        self.values.get(index)
    }

    /// The column as text (DuckDB `CAST(... AS VARCHAR)`); NULL and blobs read as `None`.
    fn text(&self, column: &str) -> Option<String> {
        match self.cell(column)? {
            Raw::Text(text) => Some(text.clone()),
            Raw::Int(number) => Some(number.to_string()),
            Raw::Real(number) => Some(number.to_string()),
            Raw::Null | Raw::Blob(_) => None,
        }
    }

    /// The column as an integer (DuckDB `CAST(... AS BIGINT)`).
    fn int(&self, column: &str) -> Option<i64> {
        match self.cell(column)? {
            Raw::Int(number) => Some(*number),
            Raw::Real(number) => Some(number.round() as i64),
            Raw::Text(text) => text.trim().parse().ok(),
            Raw::Null | Raw::Blob(_) => None,
        }
    }

    /// The column as a double (DuckDB `CAST(... AS DOUBLE)`).
    fn real(&self, column: &str) -> Option<f64> {
        match self.cell(column)? {
            Raw::Real(number) => Some(*number),
            Raw::Int(number) => Some(*number as f64),
            Raw::Text(text) => text.trim().parse().ok(),
            Raw::Null | Raw::Blob(_) => None,
        }
    }

    /// The column as a boolean. Booleans are stored as 0 and 1.
    fn flag(&self, column: &str) -> Option<bool> {
        match self.cell(column)? {
            Raw::Int(number) => Some(*number != 0),
            Raw::Real(number) => Some(*number != 0.0),
            Raw::Text(text) => match text.trim() {
                "1" | "true" | "True" => Some(true),
                "0" | "false" | "False" => Some(false),
                _ => None,
            },
            Raw::Null | Raw::Blob(_) => None,
        }
    }

    /// The column as a UTC timestamp: the stored ISO text with its offset removed, read as UTC.
    /// A value that does not read is NULL (`TRY_CAST`).
    fn moment(&self, column: &str) -> Option<Timestamp> {
        let value = self.text(column)?;
        let naive = value.replace("+00:00", "");
        Timestamp::parse_iso(&format!("{naive}Z"))
            .or_else(|_| Timestamp::parse_iso(&naive))
            .ok()
    }
}

/// The rows of `table` in the copy, in order, as records. An absent table has no rows.
fn records<'a>(sources: &'a [AnalyticsSource], table: &str) -> Vec<Record<'a>> {
    sources
        .iter()
        .filter(|source| source.table == table)
        .flat_map(|source| {
            source.rows.iter().map(|values| Record {
                columns: &source.columns,
                values,
            })
        })
        .collect()
}

/// The typed rows of `table`, built by `row`.
fn table<T>(
    sources: &[AnalyticsSource],
    name: &str,
    row: impl Fn(Record<'_>) -> T,
) -> VectorSync<T> {
    records(sources, name).into_iter().map(row).collect()
}

fn subagent_run(r: Record<'_>) -> SubagentRunRow {
    SubagentRunRow {
        id: r.text("id"),
        task_id: r.text("task_id"),
        agent_type: r.text("agent_type"),
        model: r.text("model"),
        requested_model: r.text("requested_model"),
        started_at: r.moment("started_at"),
        stopped_at: r.moment("stopped_at"),
        usage: r.text("usage"),
        telemetry: r.text("telemetry"),
    }
}

fn task(r: Record<'_>) -> TaskRow {
    TaskRow {
        id: r.text("id"),
        workspace_id: r.text("workspace_id"),
        number: r.int("number"),
        key: r.text("key"),
        title: r.text("title"),
        status: r.text("status"),
        priority: r.text("priority"),
        category: r.text("category"),
        parent_id: r.text("parent_id"),
        assignee: r.text("assignee"),
        assigned_agent: r.text("assigned_agent"),
        created_at: r.moment("created_at"),
        updated_at: r.moment("updated_at"),
        started_at: r.moment("started_at"),
        completed_at: r.moment("completed_at"),
        accepted_at: r.moment("accepted_at"),
        archived_at: r.moment("archived_at"),
        auto_execute: r.flag("auto_execute"),
    }
}

fn attempt(r: Record<'_>) -> AttemptRow {
    AttemptRow {
        id: r.text("id"),
        workspace_id: r.text("workspace_id"),
        task_id: r.text("task_id"),
        number: r.int("number"),
        status: r.text("status"),
        provider: r.text("provider"),
        adapter: r.text("adapter"),
        model: r.text("model"),
        parent_attempt_id: r.text("parent_attempt_id"),
        retry_of_attempt_id: r.text("retry_of_attempt_id"),
        rerun_of_attempt_id: r.text("rerun_of_attempt_id"),
        created_at: r.moment("created_at"),
        queued_at: r.moment("queued_at"),
        started_at: r.moment("started_at"),
        finished_at: r.moment("finished_at"),
        queue_wait_ms: r.int("queue_wait_ms"),
        duration_ms: r.int("duration_ms"),
        exit_reason: r.text("exit_reason"),
        usage: r.text("usage"),
        usage_source: r.text("usage_source"),
        cost_usd: r.real("cost_usd"),
        cost_source: r.text("cost_source"),
        modified_repository_ids: r.text("modified_repository_ids"),
    }
}

fn commit(r: Record<'_>) -> CommitRow {
    CommitRow {
        id: r.text("id"),
        repository_id: r.text("repository_id"),
        sha: r.text("sha"),
        author_name: r.text("author_name"),
        author_email: r.text("author_email"),
        committed_at: r.moment("committed_at"),
        subject: r.text("subject"),
        is_merge: r.flag("is_merge"),
        patch_id: r.text("patch_id"),
        insertions: r.int("insertions"),
        deletions: r.int("deletions"),
        files_changed: r.int("files_changed"),
        reachable: r.flag("reachable"),
    }
}

fn verification_run(r: Record<'_>) -> VerificationRun {
    VerificationRun {
        id: r.text("id"),
        workspace_id: r.text("workspace_id"),
        task_id: r.text("task_id"),
        attempt_id: r.text("attempt_id"),
        status: r.text("status"),
        created_at: r.moment("created_at"),
    }
}

fn check_result(r: Record<'_>) -> CheckResult {
    CheckResult {
        run_id: r.text("run_id"),
        name: r.text("name"),
        kind: r.text("kind"),
        repository_id: r.text("repository_id"),
        status: r.text("status"),
        created_at: r.moment("created_at"),
        failure_signatures: r.text("failure_signatures"),
    }
}

fn surface(r: Record<'_>) -> Surface {
    Surface {
        task_id: r.text("task_id"),
        files_changed: r.int("files_changed"),
        insertions: r.int("insertions"),
        deletions: r.int("deletions"),
        renames: r.int("renames"),
        binary_files: r.int("binary_files"),
        functions_affected: r.int("functions_affected"),
        types_affected: r.int("types_affected"),
    }
}

fn event(r: Record<'_>) -> Event {
    Event {
        event_type: r.text("event_type"),
        entity_id: r.text("entity_id"),
        occurred_at: r.moment("occurred_at"),
        data: r.text("data"),
    }
}

/// The analytics source rows of the copies: every table the warehouse reads, as the typed rows
/// `wg-analytics` builds its fact tables from. `subagent_runs` is copied by its own statement
/// ([`crate::service::reads::subagent_runs_source`]).
#[must_use]
pub fn analytics_input(sources: &[AnalyticsSource]) -> AnalyticsInput {
    AnalyticsInput {
        tasks: table(sources, "tasks", task),
        attempts: table(sources, "agent_attempts", attempt),
        commits: table(sources, "commits", commit),
        links: table(sources, "commit_task_links", |r| CommitLink {
            commit_id: r.text("commit_id"),
            task_id: r.text("task_id"),
            confidence: r.real("confidence"),
        }),
        file_changes: table(sources, "commit_file_changes", |r| FileChange {
            commit_id: r.text("commit_id"),
            path: r.text("path"),
            insertions: r.int("insertions"),
            deletions: r.int("deletions"),
            module: r.text("module"),
            file_class: r.text("file_class"),
        }),
        analyses: table(sources, "commit_analyses", |r| Analysis {
            commit_id: r.text("commit_id"),
            functions_affected: r.int("functions_affected"),
            types_affected: r.int("types_affected"),
        }),
        surfaces: table(sources, "task_change_surfaces", surface),
        task_projects: table(sources, "task_projects", |r| TaskProject {
            task_id: r.text("task_id"),
            project_id: r.text("project_id"),
        }),
        task_repositories: table(sources, "task_repositories", |r| TaskRepository {
            task_id: r.text("task_id"),
            repository_id: r.text("repository_id"),
        }),
        verification_runs: table(sources, "verification_runs", verification_run),
        check_results: table(sources, "verification_check_results", check_result),
        relations: table(sources, "task_relations", |r| TaskRelation {
            source_task_id: r.text("source_task_id"),
            target_task_id: r.text("target_task_id"),
            relation_type: r.text("relation_type"),
            created_at: r.moment("created_at"),
        }),
        projects: table(sources, "projects", |r| ProjectRow {
            id: r.text("id"),
            workspace_id: r.text("workspace_id"),
            slug: r.text("slug"),
        }),
        repositories: table(sources, "repositories", |r| RepositoryRow {
            id: r.text("id"),
            workspace_id: r.text("workspace_id"),
            name: r.text("name"),
        }),
        events: table(sources, "events", event),
        subagent_runs: table(sources, "subagent_runs", subagent_run),
        acceptances: table(sources, "task_acceptances", |r| AcceptanceRow {
            task_id: r.text("task_id"),
            decision: r.text("decision"),
        }),
        repository_dependencies: table(sources, "repository_dependencies", |r| {
            RepositoryDependency {
                from_repository_id: r.text("from_repository_id"),
                to_repository_id: r.text("to_repository_id"),
            }
        }),
        task_dependencies: table(sources, "task_dependencies", |r| TaskDependency {
            task_id: r.text("task_id"),
            depends_on_id: r.text("depends_on_id"),
        }),
    }
}
