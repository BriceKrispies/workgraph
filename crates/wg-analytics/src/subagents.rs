//! `analytics/subagents.py`: subagent runs by period and model, with the outcomes of their linked
//! tasks and their size bands (`analytics subagents`, M61, C014).
//!
//! A run is linked when its `task_id` names a task in the reported workspace. A task's outcome is its
//! status and its acceptance decisions; its size is its change surface when one exists, otherwise its
//! linked non-merge commits. Runs with no link are counted only when the reported workspace is the
//! default one. The shell resolves the workspace and passes the rows in [`AnalyticsInput`].

use serde_json::{Value, json};
use wg_experiments::stats::{Z_95, wilson};
use wg_pynum::{group_sorted, sorted_distinct, stable_sort_by};
use wg_types::Timestamp;

use crate::agg::{Groups, aggregate, lookup};
use crate::common::{
    Kind, Scope, Verdict, Window, iso_value, json_object, median_json, model_name, number, object,
    task_index, truthy,
};
use crate::source::{AnalyticsInput, CommitRow, SubagentRunRow, TaskRow};

/// The `period` argument: runs bucketed by ISO week or by calendar month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Week,
    Month,
}

impl Period {
    /// The period names the report accepts.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }

    /// The name the report echoes.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// `_bucket`: `YYYY-Www` for weeks (ISO year and week), `YYYY-MM` for months, in UTC.
    fn bucket(self, at: Timestamp) -> String {
        let date = at.to_offset_datetime();
        match self {
            Self::Week => {
                let (year, week, _) = date.date().to_iso_week_date();
                format!("{year}-W{week:02}")
            }
            Self::Month => format!("{:04}-{:02}", date.year(), u8::from(date.month())),
        }
    }
}

/// The outcome of a task (`_outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Rejected,
    Failed,
    Pending,
}

fn outcome(status: Option<&str>, decisions: &[String]) -> Outcome {
    if status == Some("accepted") {
        Outcome::Accepted
    } else if decisions.iter().any(|decision| decision != "accepted") {
        Outcome::Rejected
    } else if matches!(status, Some("failed" | "cancelled")) {
        Outcome::Failed
    } else {
        Outcome::Pending
    }
}

fn outcome_of(outcomes: &[(String, Outcome)], task: &str) -> Outcome {
    outcomes
        .iter()
        .find(|(key, _)| key.as_str() == task)
        .map_or(Outcome::Pending, |(_, outcome)| *outcome)
}

/// The size of a task: its change lines, and the band `bit_length - 1` of those lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Size {
    lines: Option<i64>,
    band: Option<i64>,
}

fn band_of(lines: Option<i64>) -> Option<i64> {
    lines
        .filter(|lines| *lines >= 1)
        .map(|lines| 63 - i64::from(lines.leading_zeros()))
}

fn size_of(sizes: &[(String, Size)], task: &str) -> Size {
    sizes.iter().find(|(key, _)| key.as_str() == task).map_or(
        Size {
            lines: None,
            band: None,
        },
        |(_, size)| *size,
    )
}

/// Lines of a commit that is not a merge; `None` for a merge.
fn non_merge_lines(commit: &CommitRow) -> Option<i64> {
    (commit.is_merge != Some(true))
        .then(|| commit.insertions.unwrap_or(0) + commit.deletions.unwrap_or(0))
}

/// `_sizes`: the size of each task. Its change surface wins; without one, the sum over its distinct
/// linked non-merge commits; without either, the size is unknown.
fn sizes(input: &AnalyticsInput, tasks: &[String]) -> Vec<(String, Size)> {
    let surfaces: Groups<Option<String>, i64> = aggregate(
        input.surfaces.iter(),
        |surface| surface.task_id.clone(),
        |surface| surface.insertions.unwrap_or(0) + surface.deletions.unwrap_or(0),
        |a, b| a + b,
    );
    let commits: Groups<Option<String>, CommitRow> = aggregate(
        input.commits.iter().cloned(),
        |commit| commit.id.clone(),
        |commit| commit.clone(),
        |first, _| first,
    );
    tasks
        .iter()
        .map(|task| {
            let linked = sorted_distinct(
                input
                    .links
                    .iter()
                    .filter(|link| link.task_id.as_deref() == Some(task.as_str()))
                    .filter_map(|link| link.commit_id.clone()),
            );
            let commit_lines: Vec<i64> = linked
                .iter()
                .filter_map(|id| lookup(&commits, &Some(id.clone())).and_then(non_merge_lines))
                .collect();
            let lines = match lookup(&surfaces, &Some(task.clone())) {
                Some(total) => Some(*total),
                None if commit_lines.is_empty() => None,
                None => Some(commit_lines.iter().sum()),
            };
            (
                task.clone(),
                Size {
                    lines,
                    band: band_of(lines),
                },
            )
        })
        .collect()
}

/// One run that the report counts.
#[derive(Debug, Clone, PartialEq)]
struct Run {
    model: String,
    at: Timestamp,
    task_id: Option<String>,
    output_tokens: Option<f64>,
    turns: Option<f64>,
    tool_errors: i64,
}

fn run_of(
    row: &SubagentRunRow,
    name: &str,
    at: Timestamp,
    telemetry: &serde_json::Map<String, Value>,
    task_id: Option<String>,
) -> Run {
    let usage = json_object(row.usage.as_deref());
    Run {
        model: name.to_owned(),
        at,
        task_id,
        output_tokens: number(usage.get("output_tokens")),
        turns: number(telemetry.get("turns")),
        tool_errors: number(telemetry.get("tool_errors")).unwrap_or(0.0) as i64,
    }
}

/// The verdict on one stopped run (the body of `SubagentReport._runs`).
fn classify(
    row: &SubagentRunRow,
    tasks: &Groups<Option<String>, TaskRow>,
    scope: &Scope,
    window: &Window,
) -> Option<Verdict<Run>> {
    let at = row.stopped_at?;
    if !window.contains(at) {
        return None;
    }
    let name = model_name(&row.model, &row.requested_model);
    if !window.model_matches(name) {
        return None;
    }
    let telemetry = json_object(row.telemetry.as_deref());
    // Experiment trials have their own report and never reach acceptance, so they would read as pending.
    if truthy(telemetry.get("trial_ids")) {
        return Some(Verdict {
            unlinked: false,
            kind: Kind::Trial,
        });
    }
    match lookup(tasks, &row.task_id) {
        Some(task) if !scope.owns(task) => Some(Verdict {
            unlinked: false,
            kind: Kind::Outside,
        }),
        Some(_) => Some(Verdict {
            unlinked: false,
            kind: Kind::Kept(run_of(row, name, at, &telemetry, row.task_id.clone())),
        }),
        None => Some(Verdict {
            unlinked: true,
            kind: if scope.is_default() {
                Kind::Kept(run_of(row, name, at, &telemetry, None))
            } else {
                Kind::NotDefault
            },
        }),
    }
}

/// The counted runs of one model, with the tasks they touch.
struct ModelRuns {
    name: String,
    runs: Vec<Run>,
    tasks: Vec<String>,
    /// Runs per linked task (`run_counts` in Python).
    counts: Vec<(String, usize)>,
}

impl ModelRuns {
    fn new(name: String, runs: Vec<Run>) -> Self {
        let counts: Vec<(String, usize)> = group_sorted(
            runs.iter()
                .filter_map(|run| run.task_id.clone().map(|task| (task, ()))),
        )
        .into_iter()
        .map(|(task, group)| (task, group.len()))
        .collect();
        let tasks = counts.iter().map(|(task, _)| task.clone()).collect();
        Self {
            name,
            runs,
            tasks,
            counts,
        }
    }
}

fn count_of(counts: &[(String, usize)], task: &str) -> usize {
    counts
        .iter()
        .find(|(key, _)| key.as_str() == task)
        .map_or(0, |(_, count)| *count)
}

/// Acceptance rate and its Wilson interval; both `null` with no decided task.
fn rate(accepted: usize, decided: usize) -> (Value, Value) {
    if decided == 0 {
        return (Value::Null, Value::Null);
    }
    let (low, high) = wilson(accepted as i64, decided as i64, Z_95).unwrap_or((0.0, 1.0));
    (json!(accepted as f64 / decided as f64), json!([low, high]))
}

/// The tallies of a set of tasks: accepted, rejected, failed and pending.
struct Tally {
    accepted: usize,
    rejected: usize,
    failed: usize,
    pending: usize,
}

impl Tally {
    fn of(tasks: &[String], outcomes: &[(String, Outcome)]) -> Self {
        let count = |wanted: Outcome| {
            tasks
                .iter()
                .filter(|task| outcome_of(outcomes, task) == wanted)
                .count()
        };
        Self {
            accepted: count(Outcome::Accepted),
            rejected: count(Outcome::Rejected),
            failed: count(Outcome::Failed),
            pending: count(Outcome::Pending),
        }
    }

    fn decided(&self) -> usize {
        self.accepted + self.rejected + self.failed
    }
}

/// The measured fields of a period bucket or a model total (`_summary`).
fn summary(
    runs: &[Run],
    tasks: &[String],
    outcomes: &[(String, Outcome)],
    counts: &[(String, usize)],
) -> Vec<(&'static str, Value)> {
    let tally = Tally::of(tasks, outcomes);
    let (acceptance_rate, interval) = rate(tally.accepted, tally.decided());
    let first_pass = tasks
        .iter()
        .filter(|task| {
            outcome_of(outcomes, task) == Outcome::Accepted && count_of(counts, task) == 1
        })
        .count();
    vec![
        ("runs", json!(runs.len())),
        (
            "linked_runs",
            json!(runs.iter().filter(|run| run.task_id.is_some()).count()),
        ),
        ("tasks", json!(tasks.len())),
        ("accepted", json!(tally.accepted)),
        ("rejected", json!(tally.rejected)),
        ("failed", json!(tally.failed)),
        ("pending", json!(tally.pending)),
        ("acceptance_rate", acceptance_rate),
        ("wilson", interval),
        ("first_pass", json!(first_pass)),
        (
            "median_output_tokens",
            median_json(runs.iter().filter_map(|run| run.output_tokens)),
        ),
        (
            "median_turns",
            median_json(runs.iter().filter_map(|run| run.turns)),
        ),
        (
            "tool_errors",
            json!(runs.iter().map(|run| run.tool_errors).sum::<i64>()),
        ),
    ]
}

/// `_by_period` for one model: a bucket holds the runs stopped in it; a task belongs to the bucket of
/// its latest run of the model (the first one on a tie).
fn by_period_rows(
    model: &ModelRuns,
    period: Period,
    outcomes: &[(String, Outcome)],
) -> Vec<(String, String, Value)> {
    let buckets = group_sorted(
        model
            .runs
            .iter()
            .map(|run| (period.bucket(run.at), run.clone())),
    );
    let latest = model.tasks.iter().filter_map(|task| {
        let best = model
            .runs
            .iter()
            .filter(|run| run.task_id.as_deref() == Some(task.as_str()))
            .fold(None::<&Run>, |best, run| match best {
                Some(current) if run.at <= current.at => Some(current),
                _ => Some(run),
            });
        best.map(|run| (period.bucket(run.at), task.clone()))
    });
    let task_buckets = group_sorted(latest);
    buckets
        .into_iter()
        .map(|(key, bucket_runs)| {
            let tasks = task_buckets
                .iter()
                .find(|(bucket, _)| *bucket == key)
                .map(|(_, tasks)| sorted_distinct(tasks.iter().cloned()))
                .unwrap_or_default();
            let prefix = [("period", json!(key)), ("model", json!(model.name))];
            let entries: Vec<(&str, Value)> = prefix
                .into_iter()
                .chain(summary(&bucket_runs, &tasks, outcomes, &model.counts))
                .collect();
            (key.clone(), model.name.clone(), object(entries))
        })
        .collect()
}

/// `_by_size` for one model: the tasks grouped by size band, unknown sizes last.
fn by_size_rows(
    model: &ModelRuns,
    sizes: &[(String, Size)],
    outcomes: &[(String, Outcome)],
) -> Vec<Value> {
    let bands = group_sorted(model.tasks.iter().map(|task| {
        let band = size_of(sizes, task).band;
        ((band.is_none(), band.unwrap_or(0)), task.clone())
    }));
    bands
        .into_iter()
        .map(|((unknown, band), tasks)| {
            let tally = Tally::of(&tasks, outcomes);
            let (acceptance_rate, interval) = rate(tally.accepted, tally.decided());
            let median_lines = if unknown {
                Value::Null
            } else {
                median_json(
                    tasks
                        .iter()
                        .filter_map(|task| size_of(sizes, task).lines)
                        .map(|lines| lines as f64),
                )
            };
            object(vec![
                ("model", json!(model.name)),
                ("band", if unknown { Value::Null } else { json!(band) }),
                ("tasks", json!(tasks.len())),
                ("accepted", json!(tally.accepted)),
                ("decided", json!(tally.decided())),
                ("acceptance_rate", acceptance_rate),
                ("wilson", interval),
                ("median_lines", median_lines),
            ])
        })
        .collect()
}

/// The subagent report of [`AnalyticsInput`] for the scope and window (`SubagentReport.report`).
#[must_use]
pub fn report(input: &AnalyticsInput, scope: &Scope, period: Period, window: &Window) -> Value {
    let tasks = task_index(input);
    let without_stop = input
        .subagent_runs
        .iter()
        .filter(|row| row.stopped_at.is_none())
        .count();
    let stopped: Vec<&SubagentRunRow> = input
        .subagent_runs
        .iter()
        .filter(|row| row.stopped_at.is_some())
        .collect();

    let workspace_tasks: Vec<String> = sorted_distinct(
        stopped
            .iter()
            .filter_map(|row| row.task_id.clone())
            .filter(|id| lookup(&tasks, &Some(id.clone())).is_some_and(|task| scope.owns(task))),
    );
    let decisions = group_sorted(input.acceptances.iter().filter_map(|acceptance| {
        acceptance
            .task_id
            .clone()
            .map(|task| (task, acceptance.decision.clone().unwrap_or_default()))
    }));
    let outcomes: Vec<(String, Outcome)> = workspace_tasks
        .iter()
        .map(|task| {
            let status = lookup(&tasks, &Some(task.clone())).and_then(|row| row.status.as_deref());
            let made: &[String] = decisions
                .iter()
                .find(|(key, _)| key.as_str() == task.as_str())
                .map_or(&[], |(_, list)| list.as_slice());
            (task.clone(), outcome(status, made))
        })
        .collect();
    let task_sizes = sizes(input, &workspace_tasks);

    let verdicts: Vec<Verdict<Run>> = stopped
        .iter()
        .filter_map(|row| classify(row, &tasks, scope, window))
        .collect();
    let trials = verdicts
        .iter()
        .filter(|verdict| matches!(verdict.kind, Kind::Trial))
        .count();
    let outside = verdicts
        .iter()
        .filter(|verdict| matches!(verdict.kind, Kind::Outside))
        .count();
    let unlinked = verdicts.iter().filter(|verdict| verdict.unlinked).count();
    let runs: Vec<Run> = verdicts
        .into_iter()
        .filter_map(|verdict| match verdict.kind {
            Kind::Kept(run) => Some(run),
            _ => None,
        })
        .collect();

    let models: Vec<ModelRuns> = group_sorted(runs.into_iter().map(|run| (run.model.clone(), run)))
        .into_iter()
        .map(|(name, group)| ModelRuns::new(name, group))
        .collect();

    let unknown_sizes: usize = models
        .iter()
        .map(|model| {
            model
                .tasks
                .iter()
                .filter(|task| size_of(&task_sizes, task).lines.is_none())
                .count()
        })
        .sum();

    let note_slots = [
        (without_stop > 0).then(|| {
            format!("{without_stop} subagent runs have no stopped_at and are excluded from the report")
        }),
        (unlinked > 0).then(|| {
            format!(
                "{unlinked} runs have no Task-ID link; start a subagent prompt with `Task-ID: <KEY>` to link it"
            )
        }),
        (unlinked > 0 && !scope.is_default())
            .then(|| "unlinked runs are counted only when the default workspace is reported".to_owned()),
        (outside > 0)
            .then(|| format!("{outside} runs are linked to tasks in other workspaces and are excluded")),
        (trials > 0).then(|| {
            format!(
                "{trials} runs are experiment trials (their prompt carries a Trial-ID) and are excluded; see `workgraph experiment report`"
            )
        }),
        (unknown_sizes > 0).then(|| {
            format!("{unknown_sizes} linked tasks have no change surface or linked commits, so their size is unknown")
        }),
        Some(
            "A task's outcome is its current status: accepted, rejected (a non-accepted acceptance decision), failed or cancelled; anything else is pending"
                .to_owned(),
        ),
    ];
    let notes: Vec<String> = note_slots.into_iter().flatten().collect();

    let period_rows = stable_sort_by(
        models
            .iter()
            .flat_map(|model| by_period_rows(model, period, &outcomes)),
        |a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())),
    )
    .into_iter()
    .map(|(_, _, row)| row)
    .collect();
    let size_rows: Vec<Value> = models
        .iter()
        .flat_map(|model| by_size_rows(model, &task_sizes, &outcomes))
        .collect();
    let totals: Vec<Value> = models
        .iter()
        .map(|model| {
            let entries: Vec<(&str, Value)> = std::iter::once(("model", json!(model.name)))
                .chain(summary(&model.runs, &model.tasks, &outcomes, &model.counts))
                .collect();
            object(entries)
        })
        .collect();

    object(vec![
        ("period", json!(period.name())),
        (
            "window",
            object(vec![
                ("since", iso_value(window.since)),
                ("until", iso_value(window.until)),
            ]),
        ),
        ("model", json!(window.model)),
        ("by_period", Value::Array(period_rows)),
        ("by_size", Value::Array(size_rows)),
        ("totals", Value::Array(totals)),
        ("unlinked_runs", json!(unlinked)),
        ("notes", json!(notes)),
    ])
}
