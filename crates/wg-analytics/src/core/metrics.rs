//! The metric registry (`analytics/metrics.py` `METRICS`): the 37 metrics, each with its written
//! definition, unit, statistic, base relation, time column, value and filter. The definitions
//! are the same text as the Python registry, so the catalog is the same.

use rpds::RedBlackTreeSetSync;
use serde_json::{Map, Value};

use super::bases::{Base, Row, dimensions};

/// How a metric's rows are summarised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Statistic {
    /// The number of rows.
    Count,
    /// The number of rows and of the values among them, with the order statistics of the values.
    Distribution,
    /// The number of rows, with a total of the values.
    Sum,
    /// A numerator over a denominator.
    Ratio,
}

impl Statistic {
    /// The name the catalog uses.
    pub fn name(self) -> &'static str {
        match self {
            Statistic::Count => "count",
            Statistic::Distribution => "distribution",
            Statistic::Sum => "sum",
            Statistic::Ratio => "ratio",
        }
    }
}

/// The column a metric's time filters and `period` grouping read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeColumn {
    CreatedAt,
    CompletedAt,
    AcceptedAt,
    StartedAt,
    FinishedAt,
    CommittedAt,
}

impl TimeColumn {
    /// The column name the catalog reports.
    pub fn name(self) -> &'static str {
        match self {
            TimeColumn::CreatedAt => "created_at",
            TimeColumn::CompletedAt => "completed_at",
            TimeColumn::AcceptedAt => "accepted_at",
            TimeColumn::StartedAt => "started_at",
            TimeColumn::FinishedAt => "finished_at",
            TimeColumn::CommittedAt => "committed_at",
        }
    }
}

/// How a ratio's numerator and denominator are formed.
#[derive(Debug, Clone, Copy)]
pub enum Ratio {
    /// `COUNT(*) FILTER (WHERE hit)` over `COUNT(*)`: zero over no rows, never NULL.
    Hits(fn(&Row) -> bool),
    /// `SUM(numerator)` over `SUM(denominator)`: NULL over no rows.
    Sums {
        numerator: fn(&Row) -> Option<f64>,
        denominator: fn(&Row) -> Option<f64>,
    },
}

/// One metric of the registry.
#[derive(Debug, Clone, Copy)]
pub struct Metric {
    pub id: &'static str,
    pub family: &'static str,
    pub title: &'static str,
    pub definition: &'static str,
    pub unit: &'static str,
    pub statistic: Statistic,
    pub base: Base,
    pub time: TimeColumn,
    /// The value of a row, for the distribution and sum statistics (`NULL` rows are skipped).
    pub value: fn(&Row) -> Option<f64>,
    /// The metric's `WHERE` clause over the base row.
    pub filter: fn(&Row) -> bool,
    pub ratio: Option<Ratio>,
    pub caveats: &'static [&'static str],
}

const ACTIVE_ATTEMPT_STATUSES: [&str; 5] =
    ["succeeded", "failed", "timed_out", "cancelled", "abandoned"];

/// `status` is one of `values` (a NULL status is in none).
fn one_of(value: &Option<String>, values: &[&str]) -> bool {
    value.as_deref().is_some_and(|text| values.contains(&text))
}

fn is(value: &Option<String>, expected: &str) -> bool {
    value.as_deref() == Some(expected)
}

fn always(_: &Row) -> bool {
    true
}

fn no_value(_: &Row) -> Option<f64> {
    None
}

/// Lines added plus removed by a task's final change, NULL when either count is unknown.
fn final_lines(row: &Row) -> Option<i64> {
    row.final_insertions
        .zip(row.final_deletions)
        .map(|(added, removed)| added + removed)
}

/// Lines added plus removed by a task's distinct commits.
fn churn(row: &Row) -> i64 {
    row.churn_insertions + row.churn_deletions
}

/// The lines of one commit, NULL when either count is unknown.
fn commit_lines(row: &Row) -> Option<i64> {
    row.insertions
        .zip(row.deletions)
        .map(|(added, removed)| added + removed)
}

fn as_f64(value: Option<i64>) -> Option<f64> {
    value.map(|v| v as f64)
}

const CAVEAT_LINES: &str = "Lines changed do not measure difficulty or necessity.";
const CAVEAT_SELECTION: &str = "Agents receive different tasks; differences are not caused by the agent alone (selection bias).";

/// The registry, in the order of the Python `METRICS` list.
pub fn registry() -> Vec<Metric> {
    vec![
        // throughput
        Metric {
            id: "tasks_created",
            family: "throughput",
            title: "Tasks created",
            definition: "Number of tasks created in the period.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: always,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "tasks_completed",
            family: "throughput",
            title: "Tasks completed",
            definition: "Tasks whose completion was reported (entered awaiting verification/review), by completion time. Completion is a claim, not acceptance.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::CompletedAt,
            value: no_value,
            filter: |r| r.completed_at.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "tasks_accepted",
            family: "throughput",
            title: "Tasks accepted",
            definition: "Tasks accepted (verified or manually accepted), by acceptance time.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: no_value,
            filter: |r| r.accepted_at.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "open_backlog",
            family: "throughput",
            title: "Open tasks",
            definition: "Tasks currently not accepted or cancelled, grouped by creation period.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| {
                r.status
                    .as_deref()
                    .is_some_and(|s| s != "accepted" && s != "cancelled")
            },
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "blocked_tasks",
            family: "throughput",
            title: "Blocked tasks",
            definition: "Tasks currently in the blocked state.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| is(&r.status, "blocked"),
            ratio: None,
            caveats: &[],
        },
        // quality (reopens)
        Metric {
            id: "reopened_tasks",
            family: "quality",
            title: "Reopened tasks",
            definition: "Tasks reopened at least once after acceptance or cancellation.",
            unit: "tasks",
            statistic: Statistic::Count,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| r.reopen_count > 0,
            ratio: None,
            caveats: &[],
        },
        // execution
        Metric {
            id: "queue_wait",
            family: "execution",
            title: "Queue waiting time",
            definition: "Time between an attempt being queued and starting, per top-level attempt (measured by Workgraph).",
            unit: "ms",
            statistic: Statistic::Distribution,
            base: Base::Attempts,
            time: TimeColumn::StartedAt,
            value: |r| as_f64(r.queue_wait_ms),
            filter: |r| r.queue_wait_ms.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "execution_duration",
            family: "execution",
            title: "Active execution time",
            definition: "Measured wall-clock duration of finished top-level attempts.",
            unit: "ms",
            statistic: Statistic::Distribution,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: |r| as_f64(r.duration_ms),
            filter: |r| r.duration_ms.is_some() && r.finished_at.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "task_elapsed",
            family: "execution",
            title: "Total task elapsed time",
            definition: "Creation to acceptance, for accepted tasks.",
            unit: "ms",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: |r| as_f64(r.elapsed_ms),
            filter: |r| r.elapsed_ms.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "cycle_time",
            family: "execution",
            title: "Cycle time",
            definition: "First start (claim/run) to acceptance, for accepted tasks.",
            unit: "ms",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: |r| as_f64(r.cycle_ms),
            filter: |r| r.cycle_ms.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "attempts_per_task",
            family: "execution",
            title: "Attempts per task",
            definition: "Top-level attempts per accepted task that had attempts.",
            unit: "attempts",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: |r| Some(r.attempts as f64),
            filter: |r| r.accepted_at.is_some() && r.attempts > 0,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "retry_rate",
            family: "execution",
            title: "Retry frequency",
            definition: "Share of top-level attempts that were explicit retries.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Attempts,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: always,
            ratio: Some(Ratio::Hits(|r| r.retry)),
            caveats: &[],
        },
        Metric {
            id: "first_pass_success",
            family: "execution",
            title: "First-pass success",
            definition: "Accepted tasks (with attempts) whose first attempt succeeded and needed no further attempts.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: no_value,
            filter: |r| r.accepted_at.is_some() && r.attempts > 0,
            ratio: Some(Ratio::Hits(|r| {
                r.attempts == 1 && is(&r.first_attempt_status, "succeeded")
            })),
            caveats: &[],
        },
        Metric {
            id: "attempt_success_rate",
            family: "execution",
            title: "Attempt success rate",
            definition: "Succeeded / finished top-level attempts.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &ACTIVE_ATTEMPT_STATUSES),
            ratio: Some(Ratio::Hits(|r| is(&r.status, "succeeded"))),
            caveats: &[],
        },
        Metric {
            id: "attempt_outcomes",
            family: "execution",
            title: "Attempt outcomes",
            definition: "Top-level attempts by status (group by status).",
            unit: "attempts",
            statistic: Statistic::Count,
            base: Base::Attempts,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: always,
            ratio: None,
            caveats: &[],
        },
        // code
        Metric {
            id: "cumulative_churn",
            family: "code",
            title: "Cumulative churn",
            definition: "Lines added + removed across a task's distinct linked non-merge commits (confidence >= 0.5; cherry-picked or rewritten duplicates counted once).",
            unit: "lines",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: |r| Some(churn(r) as f64),
            filter: |r| r.commits > 0,
            ratio: None,
            caveats: &[CAVEAT_LINES],
        },
        Metric {
            id: "final_change_size",
            family: "code",
            title: "Final change size",
            definition: "Lines added + removed between the attempt baseline and the accepted revision (all repositories).",
            unit: "lines",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: |r| as_f64(final_lines(r)),
            filter: |r| r.final_files.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "files_touched",
            family: "code",
            title: "Files affected",
            definition: "Distinct file paths touched by a task's linked commits.",
            unit: "files",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: |r| Some(r.files_touched as f64),
            filter: |r| r.commits > 0,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "functions_affected",
            family: "code",
            title: "Functions affected",
            definition: "Functions/methods touched (Tree-sitter) summed over a task's analysed commits.",
            unit: "functions",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: |r| Some(r.churn_functions as f64),
            filter: |r| r.commits > 0,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "rework_ratio",
            family: "code",
            title: "Rework indicator",
            definition: "Cumulative churn divided by final change size; values well above 1 mean code was rewritten during the task.",
            unit: "ratio",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::AcceptedAt,
            value: |r| {
                final_lines(r)
                    .filter(|lines| *lines != 0)
                    .map(|lines| churn(r) as f64 / lines as f64)
            },
            filter: |r| r.commits > 0 && final_lines(r).unwrap_or(0) > 0,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "repeated_file_edits",
            family: "code",
            title: "Repeated edits",
            definition: "Files edited by more than one commit within the same task.",
            unit: "files",
            statistic: Statistic::Distribution,
            base: Base::Tasks,
            time: TimeColumn::CreatedAt,
            value: |r| Some(r.repeated_file_edits as f64),
            filter: |r| r.commits > 0,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "repository_change_volume",
            family: "code",
            title: "Repository change volume",
            definition: "Lines changed by distinct task-linked commits per repository (a commit shared by several tasks counts once).",
            unit: "lines",
            statistic: Statistic::Sum,
            base: Base::RepoCommits,
            time: TimeColumn::CommittedAt,
            value: |r| as_f64(commit_lines(r)),
            filter: |r| r.is_merge == Some(false),
            ratio: None,
            caveats: &[],
        },
        // architecture
        Metric {
            id: "module_hotspots",
            family: "architecture",
            title: "Module hotspots",
            definition: "Lines changed per module across tasks (group by module).",
            unit: "lines",
            statistic: Statistic::Sum,
            base: Base::Files,
            time: TimeColumn::CreatedAt,
            value: |r| as_f64(r.lines),
            filter: always,
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "structural_distribution",
            family: "architecture",
            title: "Change by file class",
            definition: "Lines changed by file class (production/test/config/docs).",
            unit: "lines",
            statistic: Statistic::Sum,
            base: Base::Files,
            time: TimeColumn::CreatedAt,
            value: |r| as_f64(r.lines),
            filter: always,
            ratio: None,
            caveats: &[],
        },
        // agents
        Metric {
            id: "agent_completion_rate",
            family: "agents",
            title: "Agent completion rate",
            definition: "Succeeded / finished top-level attempts, typically grouped by provider and model.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &ACTIVE_ATTEMPT_STATUSES),
            ratio: Some(Ratio::Hits(|r| is(&r.status, "succeeded"))),
            caveats: &[CAVEAT_SELECTION],
        },
        Metric {
            id: "agent_duration",
            family: "agents",
            title: "Median agent execution duration",
            definition: "Duration of succeeded top-level attempts.",
            unit: "ms",
            statistic: Statistic::Distribution,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: |r| as_f64(r.duration_ms),
            filter: |r| is(&r.status, "succeeded") && r.duration_ms.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "agent_verification_pass_rate",
            family: "agents",
            title: "Verification success",
            definition: "Passed / executed verification runs attached to attempts.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: no_value,
            filter: |r| r.verification_runs > 0,
            ratio: Some(Ratio::Sums {
                numerator: |r| Some(r.verification_passed as f64),
                denominator: |r| Some(r.verification_runs as f64),
            }),
            caveats: &[],
        },
        Metric {
            id: "input_tokens",
            family: "agents",
            title: "Input tokens (measured)",
            definition: "Provider-reported input tokens; attempts without measured usage are excluded (see sample size).",
            unit: "tokens",
            statistic: Statistic::Sum,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: |r| as_f64(r.input_tokens),
            filter: |r| r.usage_source && r.input_tokens.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "output_tokens",
            family: "agents",
            title: "Output tokens (measured)",
            definition: "Provider-reported output tokens.",
            unit: "tokens",
            statistic: Statistic::Sum,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: |r| as_f64(r.output_tokens),
            filter: |r| r.usage_source && r.output_tokens.is_some(),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "cost",
            family: "agents",
            title: "Cost (measured)",
            definition: "Provider-reported cost; never estimated by Workgraph.",
            unit: "usd",
            statistic: Statistic::Sum,
            base: Base::Attempts,
            time: TimeColumn::FinishedAt,
            value: |r| r.cost_usd,
            filter: |r| r.cost_source && r.cost_usd.is_some(),
            ratio: None,
            caveats: &[],
        },
        // quality
        Metric {
            id: "verification_pass_rate",
            family: "quality",
            title: "Verification pass rate",
            definition: "Passed / completed verification runs.",
            unit: "ratio",
            statistic: Statistic::Ratio,
            base: Base::Runs,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &["passed", "failed"]),
            ratio: Some(Ratio::Hits(|r| is(&r.status, "passed"))),
            caveats: &[],
        },
        Metric {
            id: "check_failures",
            family: "quality",
            title: "Check failures",
            definition: "Failed or errored verification checks (group by kind/check).",
            unit: "checks",
            statistic: Statistic::Count,
            base: Base::Checks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &["failed", "error"]),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "test_failures",
            family: "quality",
            title: "Test failures",
            definition: "Failed test checks.",
            unit: "checks",
            statistic: Statistic::Count,
            base: Base::Checks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &["failed", "error"]) && is(&r.kind, "test"),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "build_failures",
            family: "quality",
            title: "Build failures",
            definition: "Failed build checks.",
            unit: "checks",
            statistic: Statistic::Count,
            base: Base::Checks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &["failed", "error"]) && is(&r.kind, "build"),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "integration_failures",
            family: "quality",
            title: "Integration failures",
            definition: "Failed cross-repository integration checks.",
            unit: "checks",
            statistic: Statistic::Count,
            base: Base::Checks,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| one_of(&r.status, &["failed", "error"]) && is(&r.kind, "integration"),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "regressions",
            family: "quality",
            title: "Regressions linked to previous work",
            definition: "Tasks related to earlier work as regression_of.",
            unit: "relations",
            statistic: Statistic::Count,
            base: Base::Relations,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| is(&r.relation_type, "regression_of"),
            ratio: None,
            caveats: &[],
        },
        Metric {
            id: "follow_up_fixes",
            family: "quality",
            title: "Follow-up fixes",
            definition: "Tasks related to earlier work as follows_up.",
            unit: "relations",
            statistic: Statistic::Count,
            base: Base::Relations,
            time: TimeColumn::CreatedAt,
            value: no_value,
            filter: |r| is(&r.relation_type, "follows_up"),
            ratio: None,
            caveats: &[],
        },
    ]
}

/// The metric with this id, if any.
pub fn find(id: &str) -> Option<Metric> {
    registry().into_iter().find(|metric| metric.id == id)
}

/// The catalog entry of a metric (`metric_definition` in the Python service): its dimensions are
/// the base's dimensions and `period`, sorted.
pub fn describe(metric: &Metric) -> Value {
    let dimensions = dimensions(metric.base)
        .iter()
        .copied()
        .chain(std::iter::once("period"))
        .collect::<RedBlackTreeSetSync<&str>>()
        .into_iter()
        .map(|dim| Value::String(dim.to_string()))
        .collect();
    let entries: Map<String, Value> = [
        ("id", Value::from(metric.id)),
        ("family", Value::from(metric.family)),
        ("title", Value::from(metric.title)),
        ("definition", Value::from(metric.definition)),
        ("unit", Value::from(metric.unit)),
        ("statistic", Value::from(metric.statistic.name())),
        ("time_column", Value::from(metric.time.name())),
        ("dimensions", Value::Array(dimensions)),
        (
            "caveats",
            Value::Array(metric.caveats.iter().map(|c| Value::from(*c)).collect()),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    Value::Object(entries)
}
