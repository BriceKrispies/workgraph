//! `analytics/agent_time.py`: where subagent wall time went (`analytics time`, M62, C015) and the
//! bottleneck report (`analytics bottlenecks`, M63, C003).
//!
//! A run is timed when its telemetry carries a `time` breakdown (written by the transcript timing
//! of `wg-telemetry`). Runs are selected as the subagent report selects them. Concurrency is
//! machine-wide: it counts every stopped subagent run, in any workspace, whose interval contains an
//! instant, so a slow call shows the saturation it ran in.

use std::cmp::Ordering;

use serde_json::{Map, Value, json};
use wg_pynum::{bisect_left, bisect_right, group_sorted, median, py_sum, stable_sort_by};
use wg_telemetry::timing::{POLLING, class_names, round3};

use crate::common::{
    Kind, Scope, Verdict, Window, epoch_seconds, json_object, model_name, nonempty, number, object,
    owner, parse_epoch, task_index, truthy, window_json,
};
use crate::source::{AnalyticsInput, SubagentRunRow, TaskRow};

/// The class of a run's model time, and of its time waiting on background work.
pub const MODEL_CLASS: &str = "model";
pub const WAITING_CLASS: &str = "waiting on background";
const UNLINKED: &str = "(unlinked)";
const TOP_CLASSES: usize = 3;
const SCOPE: &str = "stopped subagent runs in every workspace on this machine";
const WAITING_NOTE: &str = "Waiting is the time an agent sat idle after ending a turn until a notification resumed it; model time is the gap before each assistant message. Shares are of accounted time (tool + model + waiting).";

/// The grouping of the `breakdown` report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Class,
    Model,
    Task,
    Description,
}

impl Group {
    /// The grouping names the report accepts.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "class" => Some(Self::Class),
            "model" => Some(Self::Model),
            "task" => Some(Self::Task),
            "description" => Some(Self::Description),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Model => "model",
            Self::Task => "task",
            Self::Description => "description",
        }
    }
}

/// One counted run with a time breakdown (`_Timed`).
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub task_key: Option<String>,
    pub description: String,
    pub model: String,
    pub time: Map<String, Value>,
}

/// The runs of a report, its notes, and the `(start, stop)` interval of every stopped run.
pub struct Loaded {
    pub runs: Vec<Timed>,
    pub notes: Vec<String>,
    pub intervals: Vec<(f64, f64)>,
}

/// `_epoch(started_at)` of the row, or of the telemetry's `started_at` when the row has none. An
/// epoch of exactly zero counts as absent, as Python's `or` treats it.
fn start_of(row: &SubagentRunRow, telemetry: &Map<String, Value>) -> Option<f64> {
    row.started_at
        .map(epoch_seconds)
        .filter(|epoch| *epoch != 0.0)
        .or_else(|| parse_epoch(telemetry.get("started_at").and_then(Value::as_str)))
}

/// The verdict on one stopped run (the body of `AgentTimeReport._load`).
fn classify(
    row: &SubagentRunRow,
    telemetry: &Map<String, Value>,
    tasks: &crate::agg::Groups<Option<String>, TaskRow>,
    scope: &Scope,
    window: &Window,
) -> Option<Verdict<Timed>> {
    let stopped = row.stopped_at?;
    if !window.contains(stopped) {
        return None;
    }
    let name = model_name(&row.model, &row.requested_model);
    if !window.model_matches(name) {
        return None;
    }
    if truthy(telemetry.get("trial_ids")) {
        return Some(Verdict {
            unlinked: false,
            kind: Kind::Trial,
        });
    }
    let linked = owner(tasks, &row.task_id);
    if linked.is_some_and(|task| !scope.owns(task)) {
        return Some(Verdict {
            unlinked: false,
            kind: Kind::Outside,
        });
    }
    let unlinked = linked.is_none();
    if unlinked && !scope.is_default() {
        return Some(Verdict {
            unlinked,
            kind: Kind::NotDefault,
        });
    }
    let time = match telemetry.get("time") {
        Some(Value::Object(time)) => time.clone(),
        _ => Map::new(),
    };
    if time.is_empty() {
        return Some(Verdict {
            unlinked,
            kind: Kind::NoBreakdown,
        });
    }
    let description = nonempty(telemetry.get("description").and_then(Value::as_str))
        .or(nonempty(row.agent_type.as_deref()))
        .unwrap_or(crate::common::UNKNOWN)
        .to_owned();
    Some(Verdict {
        unlinked,
        kind: Kind::Kept(Timed {
            task_key: linked.and_then(|task| task.key.clone()),
            description,
            model: name.to_owned(),
            time,
        }),
    })
}

/// `_load`: the timed runs of the window and scope, with the notes and the intervals.
#[must_use]
pub fn load(input: &AnalyticsInput, scope: &Scope, window: &Window) -> Loaded {
    let tasks = task_index(input);
    let stopped: Vec<(&SubagentRunRow, Map<String, Value>)> = input
        .subagent_runs
        .iter()
        .filter(|row| row.stopped_at.is_some())
        .map(|row| (row, json_object(row.telemetry.as_deref())))
        .collect();
    let intervals: Vec<(f64, f64)> = stopped
        .iter()
        .filter_map(|(row, telemetry)| {
            let stop = epoch_seconds(row.stopped_at?);
            let start = start_of(row, telemetry)?;
            Some((start, start.max(stop)))
        })
        .collect();
    let verdicts: Vec<Verdict<Timed>> = stopped
        .iter()
        .filter_map(|(row, telemetry)| classify(row, telemetry, &tasks, scope, window))
        .collect();
    let count = |wanted: fn(&Kind<Timed>) -> bool| {
        verdicts
            .iter()
            .filter(|verdict| wanted(&verdict.kind))
            .count()
    };
    let trials = count(|kind| matches!(kind, Kind::Trial));
    let outside = count(|kind| matches!(kind, Kind::Outside));
    let untimed = count(|kind| matches!(kind, Kind::NoBreakdown));
    let unlinked = verdicts.iter().filter(|verdict| verdict.unlinked).count();
    let runs: Vec<Timed> = verdicts
        .into_iter()
        .filter_map(|verdict| match verdict.kind {
            Kind::Kept(run) => Some(run),
            _ => None,
        })
        .collect();
    let notes: Vec<String> = [
        (untimed > 0).then(|| {
            format!(
                "{untimed} runs have no time breakdown; run `workgraph claude backfill-time` to compute it"
            )
        }),
        (unlinked > 0 && !scope.is_default())
            .then(|| "unlinked runs are counted only when the default workspace is reported".to_owned()),
        (outside > 0)
            .then(|| format!("{outside} runs are linked to tasks in other workspaces and are excluded")),
        (trials > 0).then(|| format!("{trials} runs are experiment trials and are excluded")),
        Some(WAITING_NOTE.to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    Loaded {
        runs,
        notes,
        intervals,
    }
}

/// A number field of a run's time breakdown; absent values read as zero (`_number(...)`).
fn time_number(time: &Map<String, Value>, key: &str) -> f64 {
    number(time.get(key)).unwrap_or(0.0)
}

/// The seconds of each class of one run, in the breakdown's key order (`_class_seconds`).
fn class_seconds(time: &Map<String, Value>) -> Vec<(String, f64)> {
    time.get("classes")
        .and_then(Value::as_object)
        .map(|classes| {
            classes
                .iter()
                .map(|(name, value)| {
                    let seconds = value
                        .as_object()
                        .and_then(|class| number(class.get("seconds")))
                        .unwrap_or(0.0);
                    (name.clone(), seconds)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The totals of a set of runs (`_totals`). Sums of no runs are Python's integer zero, which the
/// JSON form keeps as `0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Totals {
    pub runs: usize,
    pub span: f64,
    pub tool: f64,
    pub model: f64,
    pub waiting: f64,
    pub polling: f64,
}

impl Totals {
    fn accounted(&self) -> f64 {
        self.tool + self.model + self.waiting
    }

    fn json(&self) -> Value {
        let empty = self.runs == 0;
        let accounted = self.accounted();
        object(vec![
            ("runs", json!(self.runs)),
            ("span_seconds", seconds(self.span, empty)),
            ("accounted_seconds", seconds(accounted, empty)),
            ("tool_seconds", seconds(self.tool, empty)),
            ("model_seconds", seconds(self.model, empty)),
            ("waiting_seconds", seconds(self.waiting, empty)),
            ("polling_seconds", seconds(self.polling, empty)),
            ("model_share", share(self.model, accounted)),
            ("waiting_share", share(self.waiting, accounted)),
            ("polling_share", share(self.polling, accounted)),
        ])
    }
}

/// A rounded seconds value; a sum of no runs is the integer `0`.
fn seconds(value: f64, empty: bool) -> Value {
    if empty {
        json!(0)
    } else {
        json!(round3(value))
    }
}

/// `_share(part, whole)`: `part / whole` when the whole is positive, else `null`.
fn share(part: f64, whole: f64) -> Value {
    if whole > 0.0 {
        json!(part / whole)
    } else {
        Value::Null
    }
}

/// The totals of `runs`.
#[must_use]
pub fn totals(runs: &[&Timed]) -> Totals {
    let sum_of = |key: &str| py_sum(runs.iter().map(|run| time_number(&run.time, key)));
    Totals {
        runs: runs.len(),
        span: sum_of("span_seconds"),
        tool: sum_of("tool_seconds"),
        model: sum_of("model_seconds"),
        waiting: sum_of("waiting_seconds"),
        polling: py_sum(runs.iter().map(|run| {
            class_seconds(&run.time)
                .iter()
                .find(|(name, _)| name == POLLING)
                .map_or(0.0, |(_, seconds)| *seconds)
        })),
    }
}

/// One contribution to a class total: a tool class, model time, or waiting time.
struct Contribution {
    class: String,
    seconds: f64,
    calls: i64,
}

fn contributions(run: &Timed) -> Vec<Contribution> {
    let tools = run
        .time
        .get("classes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|classes| classes.iter())
        .map(|(name, value)| Contribution {
            class: name.clone(),
            seconds: value
                .as_object()
                .and_then(|class| number(class.get("seconds")))
                .unwrap_or(0.0),
            calls: value
                .as_object()
                .and_then(|class| number(class.get("calls")))
                .unwrap_or(0.0) as i64,
        });
    let model = std::iter::once(Contribution {
        class: MODEL_CLASS.to_owned(),
        seconds: time_number(&run.time, "model_seconds"),
        calls: 0,
    });
    let waiting = std::iter::once(Contribution {
        class: WAITING_CLASS.to_owned(),
        seconds: time_number(&run.time, "waiting_seconds"),
        calls: 0,
    });
    tools.chain(model).chain(waiting).collect()
}

/// The keys of `items` in first-appearance order.
fn first_seen(items: impl IntoIterator<Item = String>) -> Vec<String> {
    items.into_iter().fold(Vec::new(), |seen, item| {
        if seen.contains(&item) {
            seen
        } else {
            seen.into_iter().chain(std::iter::once(item)).collect()
        }
    })
}

fn kind_of(class: &str) -> &'static str {
    if class == MODEL_CLASS {
        "model"
    } else if class == WAITING_CLASS {
        "waiting"
    } else {
        "tool"
    }
}

/// `_class_rows`: each tool class, model time and waiting, as shares of accounted time.
#[must_use]
pub fn class_rows(runs: &[Timed]) -> Vec<Value> {
    let all: Vec<Contribution> = runs.iter().flat_map(contributions).collect();
    let keys = first_seen(all.iter().map(|contribution| contribution.class.clone()));
    let sums: Vec<(String, f64, i64)> = keys
        .into_iter()
        .map(|key| {
            let (seconds, calls) = all
                .iter()
                .filter(|contribution| contribution.class == key)
                .fold((0.0_f64, 0_i64), |(seconds, calls), contribution| {
                    (seconds + contribution.seconds, calls + contribution.calls)
                });
            (key, seconds, calls)
        })
        .collect();
    let accounted = py_sum(sums.iter().map(|(_, seconds, _)| *seconds));
    let order = class_names();
    let index = |class: &str| {
        order
            .iter()
            .position(|name| *name == class)
            .unwrap_or(order.len())
    };
    let kept: Vec<(String, f64, i64)> = sums
        .into_iter()
        .filter(|(_, seconds, calls)| *seconds > 0.0 || *calls != 0)
        .collect();
    let sorted = stable_sort_by(kept, |a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then(index(&a.0).cmp(&index(&b.0)))
    });
    sorted
        .into_iter()
        .map(|(class, seconds, calls)| {
            object(vec![
                ("class", json!(class)),
                ("kind", json!(kind_of(&class))),
                ("seconds", json!(round3(seconds))),
                ("calls", json!(calls)),
                ("share", share(seconds, accounted)),
            ])
        })
        .collect()
}

/// One row of a grouped breakdown, before its JSON form.
struct GroupRow {
    key: String,
    totals: Totals,
    top: Vec<(String, f64)>,
    classes: Vec<(String, f64)>,
}

fn group_key(run: &Timed, by: Group) -> String {
    match by {
        Group::Task => run.task_key.clone().unwrap_or_else(|| UNLINKED.to_owned()),
        Group::Model => run.model.clone(),
        Group::Description | Group::Class => run.description.clone(),
    }
}

/// `_group_rows`: the runs of each model, task or description, sorted by span then key.
#[must_use]
pub fn group_rows(runs: &[Timed], by: Group) -> Vec<Value> {
    let groups = group_sorted(runs.iter().map(|run| (group_key(run, by), run)));
    let rows: Vec<GroupRow> = groups
        .into_iter()
        .map(|(key, members)| {
            let all: Vec<(String, f64)> = members
                .iter()
                .flat_map(|run| class_seconds(&run.time))
                .collect();
            let classes: Vec<(String, f64)> = first_seen(all.iter().map(|(name, _)| name.clone()))
                .into_iter()
                .map(|name| {
                    let seconds = all
                        .iter()
                        .filter(|(class, _)| *class == name)
                        .fold(0.0_f64, |acc, (_, seconds)| acc + seconds);
                    (name, seconds)
                })
                .collect();
            let top = stable_sort_by(classes.clone(), |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal)
            });
            GroupRow {
                key,
                totals: totals(&members),
                top,
                classes,
            }
        })
        .collect();
    let sorted = stable_sort_by(rows, |a, b| {
        round3(b.totals.span)
            .partial_cmp(&round3(a.totals.span))
            .unwrap_or(Ordering::Equal)
            .then(a.key.cmp(&b.key))
    });
    sorted
        .into_iter()
        .map(|row| {
            let accounted = row.totals.accounted();
            let top_classes: Vec<Value> = row
                .top
                .iter()
                .take(TOP_CLASSES)
                .map(|(class, seconds)| {
                    object(vec![
                        ("class", json!(class)),
                        ("seconds", json!(round3(*seconds))),
                    ])
                })
                .collect();
            let classes: Vec<(&str, Value)> = row
                .classes
                .iter()
                .map(|(class, seconds)| (class.as_str(), json!(round3(*seconds))))
                .collect();
            object(vec![
                ("key", json!(row.key)),
                ("runs", json!(row.totals.runs)),
                ("span_seconds", json!(round3(row.totals.span))),
                ("accounted_seconds", json!(round3(accounted))),
                ("model_seconds", json!(round3(row.totals.model))),
                ("waiting_seconds", json!(round3(row.totals.waiting))),
                ("polling_share", share(row.totals.polling, accounted)),
                ("top_classes", Value::Array(top_classes)),
                ("classes", object(classes)),
            ])
        })
        .collect()
}

/// `AgentTimeReport.breakdown`: the share of wall time by tool class, or per group.
#[must_use]
pub fn breakdown(input: &AnalyticsInput, scope: &Scope, window: &Window, by: Group) -> Value {
    let loaded = load(input, scope, window);
    let refs: Vec<&Timed> = loaded.runs.iter().collect();
    let rows = match by {
        Group::Class => class_rows(&loaded.runs),
        other => group_rows(&loaded.runs, other),
    };
    object(vec![
        ("by", json!(by.name())),
        ("window", window_json(window)),
        ("model", json!(window.model)),
        ("totals", totals(&refs).json()),
        ("notes", json!(loaded.notes)),
        ("rows", Value::Array(rows)),
    ])
}

/// A slowest call before its concurrency is read off the intervals.
struct Call {
    seconds: f64,
    at_epoch: Option<f64>,
    class: String,
    /// The call's `tool`, `command` and `at` as the breakdown stores them.
    tool: Value,
    command: Value,
    at: Value,
    description: String,
    model: String,
    task: Option<String>,
}

/// The integer median of concurrency counts: an odd count gives an integer, an even one a float.
fn int_median(values: &[i64]) -> Value {
    match median(values.iter().map(|value| *value as f64)) {
        None => Value::Null,
        Some(value) if values.len() % 2 == 1 => json!(value as i64),
        Some(value) => json!(value),
    }
}

/// `AgentTimeReport.bottlenecks`: time by class, the slowest calls with the runs active at their
/// start, and wall time per task. `top` bounds the slowest calls and the task rows.
#[must_use]
pub fn bottlenecks(input: &AnalyticsInput, scope: &Scope, window: &Window, top: usize) -> Value {
    let loaded = load(
        input,
        scope,
        &Window {
            model: None,
            ..window.clone()
        },
    );
    let starts = stable_sort_by(loaded.intervals.iter().map(|(start, _)| *start), |a, b| {
        a.total_cmp(b)
    });
    let stops = stable_sort_by(loaded.intervals.iter().map(|(_, stop)| *stop), |a, b| {
        a.total_cmp(b)
    });
    // Runs that started by `at` and had not stopped before it.
    let active =
        |at: f64| -> i64 { bisect_right(&starts, at) as i64 - bisect_left(&stops, at) as i64 };
    let since = window.since.map(epoch_seconds);
    let until = window.until.map(epoch_seconds);
    let in_window = |at: f64| since.is_none_or(|s| at >= s) && until.is_none_or(|u| at < u);

    let calls: Vec<Call> = loaded
        .runs
        .iter()
        .flat_map(|run| {
            run.time
                .get("slowest")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .map(move |call| {
                    let at = call.get("at").cloned().unwrap_or(Value::Null);
                    let class = nonempty(call.get("class").and_then(Value::as_str))
                        .unwrap_or(crate::common::UNKNOWN)
                        .to_owned();
                    Call {
                        seconds: number(call.get("seconds")).unwrap_or(0.0),
                        at_epoch: parse_epoch(at.as_str()),
                        class,
                        tool: call.get("tool").cloned().unwrap_or(Value::Null),
                        command: call.get("command").cloned().unwrap_or(Value::Null),
                        at,
                        description: run.description.clone(),
                        model: run.model.clone(),
                        task: run.task_key.clone(),
                    }
                })
        })
        .collect();
    let slowest: Vec<Call> = stable_sort_by(calls, |a, b| b.seconds.total_cmp(&a.seconds))
        .into_iter()
        .take(top)
        .collect();

    let concurrent: Vec<i64> = slowest
        .iter()
        .filter_map(|call| call.at_epoch.map(active))
        .collect();
    let peak = loaded
        .intervals
        .iter()
        .filter(|(start, _)| in_window(*start))
        .map(|(start, _)| active(*start))
        .max()
        .unwrap_or(0);

    let refs: Vec<&Timed> = loaded.runs.iter().collect();
    let slowest_json: Vec<Value> = slowest
        .iter()
        .map(|call| {
            object(vec![
                ("class", json!(call.class)),
                ("seconds", json!(call.seconds)),
                ("tool", call.tool.clone()),
                ("command", call.command.clone()),
                ("at", call.at.clone()),
                ("description", json!(call.description)),
                ("model", json!(call.model)),
                ("task", json!(call.task)),
                (
                    "concurrent_runs",
                    call.at_epoch.map_or(Value::Null, |at| json!(active(at))),
                ),
            ])
        })
        .collect();
    let by_task: Vec<Value> = group_rows(&loaded.runs, Group::Task)
        .into_iter()
        .take(top)
        .collect();

    object(vec![
        ("window", window_json(window)),
        ("totals", totals(&refs).json()),
        ("classes", Value::Array(class_rows(&loaded.runs))),
        ("slowest_calls", Value::Array(slowest_json)),
        (
            "concurrency",
            object(vec![
                ("peak_during_window", json!(peak)),
                ("median_at_slowest_calls", int_median(&concurrent)),
                ("scope", json!(SCOPE)),
            ]),
        ),
        ("by_task", Value::Array(by_task)),
        ("notes", json!(loaded.notes)),
    ])
}
