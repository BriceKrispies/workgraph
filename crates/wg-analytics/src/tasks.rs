//! The task-level fact tables of `analytics/warehouse.py` `_build`: `task_surface` (the final
//! change surface of a task, summed over its repositories) and `task_facts` (one row per task with
//! its primary project, attempt totals, lifecycle times, commit churn and file rollups).

use rpds::RedBlackTreeSetSync;
use wg_types::Timestamp;

use crate::agg::{Groups, aggregate, count, lookup, max_some, min_some, sum_opt};
use crate::commits::{TaskCode, TaskFile};
use crate::source::{AnalyticsInput, AttemptRow, Event, TaskRow};

/// A row of `task_surface`: the sums of `task_change_surfaces` over one task's repositories.
/// Each sum is NULL when no repository reports the value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskSurface {
    pub task_id: Option<String>,
    pub final_files: Option<i64>,
    pub final_insertions: Option<i64>,
    pub final_deletions: Option<i64>,
    pub final_renames: Option<i64>,
    pub final_binary: Option<i64>,
    pub final_functions: Option<i64>,
    pub final_types: Option<i64>,
    pub final_repositories: i64,
}

impl TaskSurface {
    fn of(surface: &crate::source::Surface) -> Self {
        Self {
            task_id: surface.task_id.clone(),
            final_files: surface.files_changed,
            final_insertions: surface.insertions,
            final_deletions: surface.deletions,
            final_renames: surface.renames,
            final_binary: surface.binary_files,
            final_functions: surface.functions_affected,
            final_types: surface.types_affected,
            final_repositories: 1,
        }
    }

    fn merge(a: Self, b: Self) -> Self {
        Self {
            task_id: a.task_id,
            final_files: sum_opt(a.final_files, b.final_files),
            final_insertions: sum_opt(a.final_insertions, b.final_insertions),
            final_deletions: sum_opt(a.final_deletions, b.final_deletions),
            final_renames: sum_opt(a.final_renames, b.final_renames),
            final_binary: sum_opt(a.final_binary, b.final_binary),
            final_functions: sum_opt(a.final_functions, b.final_functions),
            final_types: sum_opt(a.final_types, b.final_types),
            final_repositories: a.final_repositories + b.final_repositories,
        }
    }
}

fn surface_groups(input: &AnalyticsInput) -> Groups<Option<String>, TaskSurface> {
    aggregate(
        input.surfaces.iter().cloned(),
        |surface| surface.task_id.clone(),
        TaskSurface::of,
        TaskSurface::merge,
    )
}

/// `task_surface`, one row per task id.
pub fn task_surface(input: &AnalyticsInput) -> Vec<TaskSurface> {
    surface_groups(input).values().cloned().collect()
}

/// The primary project of a task: the smallest project id among its links, and how many links it
/// has (`MIN(project_id)` and `COUNT(*)` over `task_projects`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projects {
    pub first: Option<String>,
    pub count: i64,
}

/// The project groups of `task_projects`, keyed by task id.
pub fn projects(input: &AnalyticsInput) -> Groups<Option<String>, Projects> {
    aggregate(
        input.task_projects.iter().cloned(),
        |link| link.task_id.clone(),
        |link| Projects {
            first: link.project_id.clone(),
            count: 1,
        },
        |a, b| Projects {
            first: min_some(a.first, b.first),
            count: a.count + b.count,
        },
    )
}

/// The attempt totals of one task over its top-level attempts (those without a parent attempt).
/// `first` is the status of the attempt with the lowest number, as `arg_min(status, number)`.
#[derive(Debug, Clone, Default)]
struct TopAttempts {
    attempts: i64,
    succeeded: i64,
    failed: i64,
    retries: i64,
    execution_ms: Option<i64>,
    queue_wait_ms: Option<i64>,
    first_attempt_at: Option<Timestamp>,
    first: Option<(i64, Option<String>)>,
}

impl TopAttempts {
    fn of(attempt: &AttemptRow) -> Self {
        Self {
            attempts: 1,
            succeeded: count(attempt.status.as_deref() == Some("succeeded")),
            failed: count(matches!(
                attempt.status.as_deref(),
                Some("failed" | "timed_out")
            )),
            retries: count(attempt.retry_of_attempt_id.is_some()),
            execution_ms: attempt.duration_ms,
            queue_wait_ms: attempt.queue_wait_ms,
            first_attempt_at: attempt.started_at,
            first: attempt.number.map(|n| (n, attempt.status.clone())),
        }
    }

    fn merge(a: Self, b: Self) -> Self {
        Self {
            attempts: a.attempts + b.attempts,
            succeeded: a.succeeded + b.succeeded,
            failed: a.failed + b.failed,
            retries: a.retries + b.retries,
            execution_ms: sum_opt(a.execution_ms, b.execution_ms),
            queue_wait_ms: sum_opt(a.queue_wait_ms, b.queue_wait_ms),
            first_attempt_at: min_some(a.first_attempt_at, b.first_attempt_at),
            first: match (a.first, b.first) {
                (Some(x), Some(y)) => Some(if y.0 < x.0 { y } else { x }),
                (x, None) => x,
                (None, y) => y,
            },
        }
    }
}

/// The status a `task.status_changed` event moved the task to (`json_extract_string(data, '$.to')`).
fn status_target(event: &Event) -> Option<String> {
    let data = serde_json::from_str::<serde_json::Value>(event.data.as_deref()?).ok()?;
    data.get("to")?.as_str().map(str::to_owned)
}

/// `MAX(occurred_at)` of the status changes to `accepted` or `cancelled`, per task.
fn closed_groups(input: &AnalyticsInput) -> Groups<Option<String>, Option<Timestamp>> {
    aggregate(
        input
            .events
            .iter()
            .filter(|event| {
                event.event_type.as_deref() == Some("task.status_changed")
                    && matches!(
                        status_target(event).as_deref(),
                        Some("accepted" | "cancelled")
                    )
            })
            .cloned(),
        |event| event.entity_id.clone(),
        |event| event.occurred_at,
        max_some,
    )
}

/// `COUNT(*)` of the `task.reopened` events, per task.
fn reopened_groups(input: &AnalyticsInput) -> Groups<Option<String>, i64> {
    aggregate(
        input
            .events
            .iter()
            .filter(|event| event.event_type.as_deref() == Some("task.reopened"))
            .cloned(),
        |event| event.entity_id.clone(),
        |_| 1_i64,
        |a, b| a + b,
    )
}

/// `COUNT(*)` of the repository links, per task.
fn repository_groups(input: &AnalyticsInput) -> Groups<Option<String>, i64> {
    aggregate(
        input.task_repositories.iter().cloned(),
        |link| link.task_id.clone(),
        |_| 1_i64,
        |a, b| a + b,
    )
}

/// The set of distinct paths of a task's file rows, for `COUNT(DISTINCT path)`.
fn touched_groups(files: &[TaskFile]) -> Groups<Option<String>, RedBlackTreeSetSync<String>> {
    aggregate(
        files.iter().filter(|file| file.path.is_some()).cloned(),
        |file| file.task_id.clone(),
        |file| {
            file.path
                .iter()
                .fold(RedBlackTreeSetSync::new_sync(), |set, path| {
                    set.insert(path.clone())
                })
        },
        |a, b| b.iter().fold(a, |set, path| set.insert(path.clone())),
    )
}

/// The number of a task's file rows edited more than once.
fn repeated_groups(files: &[TaskFile]) -> Groups<Option<String>, i64> {
    aggregate(
        files.iter().filter(|file| file.edits > 1).cloned(),
        |file| file.task_id.clone(),
        |_| 1_i64,
        |a, b| a + b,
    )
}

/// `task_facts`: one row per task.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskFact {
    pub task: TaskRow,
    pub project_id: Option<String>,
    pub project_count: i64,
    pub linked_repositories: i64,
    pub attempts: i64,
    pub succeeded_attempts: i64,
    pub failed_attempts: i64,
    pub retries: i64,
    pub execution_ms: Option<i64>,
    pub queue_wait_ms: Option<i64>,
    pub first_attempt_at: Option<Timestamp>,
    pub first_attempt_status: Option<String>,
    pub closed_at: Option<Timestamp>,
    pub reopen_count: i64,
    pub commits: i64,
    pub churn_insertions: i64,
    pub churn_deletions: i64,
    pub repositories_touched: i64,
    pub churn_functions: i64,
    pub final_files: Option<i64>,
    pub final_insertions: Option<i64>,
    pub final_deletions: Option<i64>,
    pub final_functions: Option<i64>,
    pub final_types: Option<i64>,
    pub files_touched: i64,
    pub repeated_file_edits: i64,
    pub elapsed_ms: Option<i64>,
    pub cycle_ms: Option<i64>,
}

/// Milliseconds since the epoch, as DuckDB's `epoch_ms`.
fn epoch_ms(at: Timestamp) -> i64 {
    let nanos = at.to_offset_datetime().unix_timestamp_nanos();
    i64::try_from(nanos.div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

/// `task_facts`, from the task-level inputs that [`crate::build`] computes first: `task_code`
/// and `task_files`. The surfaces and the project and repository links come from `input`.
pub fn task_facts(input: &AnalyticsInput, codes: &[TaskCode], files: &[TaskFile]) -> Vec<TaskFact> {
    let top_groups: Groups<Option<String>, TopAttempts> = aggregate(
        input
            .attempts
            .iter()
            .filter(|attempt| attempt.parent_attempt_id.is_none())
            .cloned(),
        |attempt| attempt.task_id.clone(),
        TopAttempts::of,
        TopAttempts::merge,
    );
    let code_groups: Groups<Option<String>, TaskCode> = aggregate(
        codes.iter().cloned(),
        |code| code.task_id.clone(),
        |code| code.clone(),
        |first, _| first,
    );
    let closed = closed_groups(input);
    let reopened = reopened_groups(input);
    let projects = projects(input);
    let repositories = repository_groups(input);
    let surfaces = surface_groups(input);
    let touched = touched_groups(files);
    let repeated = repeated_groups(files);

    input
        .tasks
        .iter()
        .map(|task| {
            let top = lookup(&top_groups, &task.id).cloned().unwrap_or_default();
            let project = lookup(&projects, &task.id);
            let code = lookup(&code_groups, &task.id);
            let surface = lookup(&surfaces, &task.id);
            TaskFact {
                task: task.clone(),
                project_id: project.and_then(|p| p.first.clone()),
                project_count: project.map_or(0, |p| p.count),
                linked_repositories: lookup(&repositories, &task.id).copied().unwrap_or(0),
                attempts: top.attempts,
                succeeded_attempts: top.succeeded,
                failed_attempts: top.failed,
                retries: top.retries,
                execution_ms: top.execution_ms,
                queue_wait_ms: top.queue_wait_ms,
                first_attempt_at: top.first_attempt_at,
                first_attempt_status: top.first.and_then(|(_, status)| status),
                closed_at: lookup(&closed, &task.id).copied().flatten(),
                reopen_count: lookup(&reopened, &task.id).copied().unwrap_or(0),
                commits: code.map_or(0, |c| c.commits),
                churn_insertions: code.map_or(0, |c| c.churn_insertions),
                churn_deletions: code.map_or(0, |c| c.churn_deletions),
                repositories_touched: code.map_or(0, |c| c.repositories_touched),
                churn_functions: code.map_or(0, |c| c.functions_affected),
                final_files: surface.and_then(|s| s.final_files),
                final_insertions: surface.and_then(|s| s.final_insertions),
                final_deletions: surface.and_then(|s| s.final_deletions),
                final_functions: surface.and_then(|s| s.final_functions),
                final_types: surface.and_then(|s| s.final_types),
                files_touched: lookup(&touched, &task.id)
                    .map_or(0, |set| i64::try_from(set.size()).unwrap_or(i64::MAX)),
                repeated_file_edits: lookup(&repeated, &task.id).copied().unwrap_or(0),
                elapsed_ms: task.accepted_at.and_then(|accepted| {
                    task.created_at
                        .map(|created| epoch_ms(accepted) - epoch_ms(created))
                }),
                cycle_ms: task.accepted_at.and_then(|accepted| {
                    task.started_at
                        .map(|started| epoch_ms(accepted) - epoch_ms(started))
                }),
            }
        })
        .collect()
}
