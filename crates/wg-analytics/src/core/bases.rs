//! The base relations of the metric registry (`analytics/metrics.py` `BASES`): each is a join of
//! the fact tables and source rows, flattened into one [`Row`] per base row. A row carries every
//! dimension and measure any metric on its base reads; the columns a base does not have stay
//! `None` and are never read by that base's metrics.

use rpds::VectorSync;
use wg_types::Timestamp;

use crate::Facts;
use crate::agg::{Groups, aggregate, lookup};
use crate::source::AnalyticsInput;
use crate::tasks::TaskFact;

/// A base relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// One row per non-archived task (`task_facts`).
    Tasks,
    /// One row per top-level agent attempt.
    Attempts,
    /// One row per verification run, with its task and attempt.
    Runs,
    /// One row per verification check result, with its run and task.
    Checks,
    /// One row per task relation.
    Relations,
    /// One row per task file edit (`task_files`).
    Files,
    /// One row per distinct commit linked to a task, in a repository of the workspace.
    RepoCommits,
}

/// The dimensions a base can be grouped and filtered by (`BASE_DIMENSIONS`).
pub fn dimensions(base: Base) -> &'static [&'static str] {
    match base {
        Base::Tasks => &[
            "project",
            "category",
            "priority",
            "status",
            "agent",
            "repository",
        ],
        Base::Attempts => &[
            "project", "category", "priority", "status", "provider", "model", "agent",
        ],
        Base::Runs => &[
            "project", "category", "status", "provider", "model", "agent",
        ],
        Base::Checks => &[
            "project",
            "category",
            "status",
            "kind",
            "check_name",
            "repository",
        ],
        Base::Relations => &["project", "category", "relation_type"],
        Base::Files => &["project", "category", "module", "repository", "file_class"],
        Base::RepoCommits => &["repository"],
    }
}

/// One row of a base relation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub ws: Option<String>,
    pub project: Option<String>,
    pub category: Option<String>,
    pub priority: Option<String>,
    pub status: Option<String>,
    pub agent: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The repository of this row. For tasks it is set only when the query groups by repository,
    /// after the join with `task_repositories` has expanded the row.
    pub repository: Option<String>,
    /// The repositories a task is linked to (`task_repositories`).
    pub repositories: VectorSync<String>,
    pub module: Option<String>,
    pub file_class: Option<String>,
    pub kind: Option<String>,
    pub check_name: Option<String>,
    pub relation_type: Option<String>,

    pub created_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub accepted_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub committed_at: Option<Timestamp>,

    pub queue_wait_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub cycle_ms: Option<i64>,
    pub attempts: i64,
    pub reopen_count: i64,
    pub commits: i64,
    pub churn_insertions: i64,
    pub churn_deletions: i64,
    pub churn_functions: i64,
    pub final_insertions: Option<i64>,
    pub final_deletions: Option<i64>,
    pub final_files: Option<i64>,
    pub files_touched: i64,
    pub repeated_file_edits: i64,
    pub first_attempt_status: Option<String>,
    /// `retry_of_attempt_id IS NOT NULL`.
    pub retry: bool,
    pub verification_runs: i64,
    pub verification_passed: i64,
    /// `usage_source IS NOT NULL`.
    pub usage_source: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// `cost_source IS NOT NULL`.
    pub cost_source: bool,
    pub cost_usd: Option<f64>,
    /// Lines added plus removed by one file edit.
    pub lines: Option<i64>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub is_merge: Option<bool>,
}

/// The rows of one base, over the whole warehouse (the caller filters by workspace).
pub fn rows(base: Base, input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    match base {
        Base::Tasks => tasks(input, facts),
        Base::Attempts => attempts(facts),
        Base::Runs => runs(input, facts),
        Base::Checks => checks(input, facts),
        Base::Relations => relations(input, facts),
        Base::Files => files(facts),
        Base::RepoCommits => repo_commits(input, facts),
    }
}

/// The task facts keyed by task id.
fn task_groups(facts: &Facts) -> Groups<Option<String>, TaskFact> {
    aggregate(
        facts.task_facts.iter().cloned(),
        |fact| fact.task.id.clone(),
        |fact| fact.clone(),
        |first, _| first,
    )
}

/// The repositories of each task, in table order.
fn repository_links(input: &AnalyticsInput) -> Groups<Option<String>, VectorSync<String>> {
    input
        .task_repositories
        .iter()
        .fold(Groups::new_sync(), |links, link| {
            match link.repository_id.clone() {
                Some(repository) => {
                    let key = link.task_id.clone();
                    let existing = links
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(VectorSync::new_sync);
                    links.insert(key, existing.push_back(repository))
                }
                None => links,
            }
        })
}

fn tasks(input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    let links = repository_links(input);
    facts
        .task_facts
        .iter()
        .filter(|fact| fact.task.archived_at.is_none())
        .map(|fact| Row {
            ws: fact.task.workspace_id.clone(),
            project: fact.project_id.clone(),
            category: fact.task.category.clone(),
            priority: fact.task.priority.clone(),
            status: fact.task.status.clone(),
            agent: fact.task.assigned_agent.clone(),
            repositories: links.get(&fact.task.id).cloned().unwrap_or_default(),
            created_at: fact.task.created_at,
            completed_at: fact.task.completed_at,
            accepted_at: fact.task.accepted_at,
            elapsed_ms: fact.elapsed_ms,
            cycle_ms: fact.cycle_ms,
            attempts: fact.attempts,
            reopen_count: fact.reopen_count,
            commits: fact.commits,
            churn_insertions: fact.churn_insertions,
            churn_deletions: fact.churn_deletions,
            churn_functions: fact.churn_functions,
            final_insertions: fact.final_insertions,
            final_deletions: fact.final_deletions,
            final_files: fact.final_files,
            files_touched: fact.files_touched,
            repeated_file_edits: fact.repeated_file_edits,
            first_attempt_status: fact.first_attempt_status.clone(),
            ..Row::default()
        })
        .collect()
}

fn attempts(facts: &Facts) -> Vec<Row> {
    facts
        .attempt_facts
        .iter()
        .filter(|fact| fact.attempt.parent_attempt_id.is_none())
        .map(|fact| Row {
            ws: fact.attempt.workspace_id.clone(),
            project: fact.project_id.clone(),
            category: fact.category.clone(),
            priority: fact.priority.clone(),
            status: fact.attempt.status.clone(),
            agent: fact.attempt.provider.clone(),
            provider: fact.attempt.provider.clone(),
            // The raw column: `af.*` also carries `model`, and the query resolves the name to it
            // rather than to the `COALESCE(model, '(unspecified)')` alias, so NULL stays NULL.
            model: fact.attempt.model.clone(),
            created_at: fact.attempt.created_at,
            started_at: fact.attempt.started_at,
            finished_at: fact.attempt.finished_at,
            queue_wait_ms: fact.attempt.queue_wait_ms,
            duration_ms: fact.attempt.duration_ms,
            retry: fact.attempt.retry_of_attempt_id.is_some(),
            verification_runs: fact.verification_runs,
            verification_passed: fact.verification_passed,
            usage_source: fact.attempt.usage_source.is_some(),
            input_tokens: fact.input_tokens,
            output_tokens: fact.output_tokens,
            cost_source: fact.attempt.cost_source.is_some(),
            cost_usd: fact.attempt.cost_usd,
            ..Row::default()
        })
        .collect()
}

fn runs(input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    let tasks = task_groups(facts);
    let attempts = aggregate(
        input.attempts.iter().cloned(),
        |attempt| attempt.id.clone(),
        |attempt| attempt.clone(),
        |first, _| first,
    );
    input
        .verification_runs
        .iter()
        .filter_map(|run| {
            let task = lookup(&tasks, &run.task_id)?;
            // A LEFT JOIN: a run without an attempt is a manual run.
            let attempt = lookup(&attempts, &run.attempt_id);
            let provider = attempt
                .and_then(|attempt| attempt.provider.clone())
                .unwrap_or_else(|| "(manual)".to_string());
            Some(Row {
                ws: run.workspace_id.clone(),
                project: task.project_id.clone(),
                category: task.task.category.clone(),
                status: run.status.clone(),
                agent: Some(provider.clone()),
                provider: Some(provider),
                model: Some(
                    attempt
                        .and_then(|attempt| attempt.model.clone())
                        .unwrap_or_else(|| "(unspecified)".to_string()),
                ),
                created_at: run.created_at,
                ..Row::default()
            })
        })
        .collect()
}

fn checks(input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    let tasks = task_groups(facts);
    let runs = aggregate(
        input.verification_runs.iter().cloned(),
        |run| run.id.clone(),
        |run| run.clone(),
        |first, _| first,
    );
    input
        .check_results
        .iter()
        .filter_map(|check| {
            let run = lookup(&runs, &check.run_id)?;
            let task = lookup(&tasks, &run.task_id)?;
            Some(Row {
                ws: run.workspace_id.clone(),
                project: task.project_id.clone(),
                category: task.task.category.clone(),
                status: check.status.clone(),
                kind: check.kind.clone(),
                check_name: check.name.clone(),
                repository: check.repository_id.clone(),
                created_at: check.created_at,
                ..Row::default()
            })
        })
        .collect()
}

fn relations(input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    let tasks = task_groups(facts);
    input
        .relations
        .iter()
        .filter_map(|relation| {
            let task = lookup(&tasks, &relation.source_task_id)?;
            Some(Row {
                ws: task.task.workspace_id.clone(),
                project: task.project_id.clone(),
                category: task.task.category.clone(),
                relation_type: relation.relation_type.clone(),
                created_at: relation.created_at,
                ..Row::default()
            })
        })
        .collect()
}

fn files(facts: &Facts) -> Vec<Row> {
    let tasks = task_groups(facts);
    facts
        .task_files
        .iter()
        .filter_map(|file| {
            let task = lookup(&tasks, &file.task_id)?;
            Some(Row {
                ws: task.task.workspace_id.clone(),
                project: task.project_id.clone(),
                category: task.task.category.clone(),
                module: file.module.clone(),
                repository: file.repository_id.clone(),
                file_class: file.file_class.clone(),
                created_at: task.task.created_at,
                accepted_at: task.task.accepted_at,
                lines: file.lines,
                ..Row::default()
            })
        })
        .collect()
}

/// `repo_commits`: the commits of `task_commits`, once each (`SELECT DISTINCT`), joined to their
/// repository for the workspace.
fn repo_commits(input: &AnalyticsInput, facts: &Facts) -> Vec<Row> {
    let workspaces = aggregate(
        input.repositories.iter().cloned(),
        |repository| repository.id.clone(),
        |repository| repository.workspace_id.clone(),
        |first, _| first,
    );
    let commits = aggregate(
        facts.task_commits.iter().map(|link| link.commit.clone()),
        |commit| commit.id.clone(),
        |commit| commit.clone(),
        |first, _| first,
    );
    commits
        .values()
        .filter_map(|commit| {
            let ws = lookup(&workspaces, &commit.repository_id)?.clone();
            Some(Row {
                ws,
                repository: commit.repository_id.clone(),
                committed_at: commit.committed_at,
                insertions: commit.insertions,
                deletions: commit.deletions,
                is_merge: commit.is_merge,
                ..Row::default()
            })
        })
        .collect()
}
