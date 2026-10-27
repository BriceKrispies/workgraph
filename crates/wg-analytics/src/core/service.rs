//! The dashboard analyses of `analytics/service.py`: `overview`, `compare` and `task_history`.
//! `overview` and `compare` are bundles of [`query::run`](super::query::run); `task_history` reads
//! the `task_facts` rows of accepted tasks. Each is a pure function of the source rows and the
//! facts, and the caller resolves the project and repository names to ids first.

use std::cmp::Reverse;

use rpds::RedBlackTreeMapSync;
use serde_json::{Value, json};
use wg_types::Timestamp;

use super::metrics::{Statistic, describe, find};
use super::period::Period;
use super::query::{Filters, Query, QueryError, run};
use crate::Facts;
use crate::source::AnalyticsInput;
use crate::tasks::TaskFact;

/// The note every analysis that compares groups carries (`CAUSATION_NOTE`).
pub const CAUSATION_NOTE: &str = "Metrics describe observed associations; differences between groups do not establish causation.";

/// A `[since, until)` window of the compare analysis.
pub type Window = (Timestamp, Timestamp);

/// The rows of a query with the given grouping and period, and no filters.
fn grouped_rows(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    metric: &str,
    group_by: &[&str],
    period: Period,
) -> Result<Vec<Value>, QueryError> {
    let query = Query {
        group_by: group_by
            .iter()
            .map(|dimension| dimension.to_string())
            .collect(),
        period,
        ..Query::new(metric)
    };
    run(input, facts, workspace, &query).map(|answer| answer.rows)
}

/// `overview`: the dashboard bundle of the common metrics, grouped by `period` buckets where the
/// metric has a time, and the 15 largest module hotspots.
pub fn overview(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    period: Period,
) -> Result<Value, QueryError> {
    let by = |metric: &str, group_by: &[&str]| -> Result<Value, QueryError> {
        grouped_rows(input, facts, workspace, metric, group_by, period).map(Value::Array)
    };
    let hotspots: Vec<Value> = grouped_rows(
        input,
        facts,
        workspace,
        "module_hotspots",
        &["module"],
        period,
    )?
    .into_iter()
    .take(15)
    .collect();
    Ok(json!({
        "created_by_period": by("tasks_created", &["period"])?,
        "accepted_by_period": by("tasks_accepted", &["period"])?,
        "completed_by_period": by("tasks_completed", &["period"])?,
        "open_by_status": by("open_backlog", &["status"])?,
        "by_category": by("tasks_created", &["category"])?,
        "execution_duration": by("execution_duration", &[])?,
        "queue_wait": by("queue_wait", &[])?,
        "task_elapsed": by("task_elapsed", &[])?,
        "first_pass_success": by("first_pass_success", &[])?,
        "agents": by("agent_completion_rate", &["provider", "model"])?,
        "agent_duration": by("agent_duration", &["provider", "model"])?,
        "churn": by("cumulative_churn", &[])?,
        "final_change": by("final_change_size", &[])?,
        "rework": by("rework_ratio", &[])?,
        "check_failures_by_period": by("check_failures", &["period", "kind"])?,
        "verification_pass_rate": by("verification_pass_rate", &["period"])?,
        "repository_volume": by("repository_change_volume", &["repository"])?,
        "module_hotspots": Value::Array(hotspots),
    }))
}

/// `compare`: one metric over two windows, grouped by the dimensions of `group_by` (`period` is
/// ignored). Each group of either window appears once, with the statistic of the metric (the
/// median for a distribution, otherwise the value), the change and the relative change.
pub fn compare(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    metric_id: &str,
    first: Window,
    second: Window,
    group_by: &[&str],
) -> Result<Value, QueryError> {
    let metric = find(metric_id).ok_or_else(|| QueryError::UnknownMetric(metric_id.to_string()))?;
    let group: Vec<String> = group_by
        .iter()
        .filter(|dimension| **dimension != "period")
        .map(|dimension| dimension.to_string())
        .collect();
    let window_rows = |(since, until): Window| -> Result<Vec<Value>, QueryError> {
        let query = Query {
            group_by: group.clone(),
            filters: Filters {
                since: Some(since),
                until: Some(until),
                ..Filters::default()
            },
            ..Query::new(metric_id)
        };
        run(input, facts, workspace, &query).map(|answer| answer.rows)
    };
    let a_rows = by_key(&window_rows(first)?, &group);
    let b_rows = by_key(&window_rows(second)?, &group);
    let key_field = if metric.statistic == Statistic::Distribution {
        "median"
    } else {
        "value"
    };
    // The union of the keys, ordered as Python's `sorted(keys, key=str)` orders the key tuples.
    let keys = a_rows.keys().chain(b_rows.keys()).fold(
        RedBlackTreeMapSync::<String, GroupKey>::new_sync(),
        |keys, key| keys.insert(tuple_repr(key), key.clone()),
    );
    let rows: Vec<Value> = keys
        .values()
        .map(|key| comparison_row(key, &group, a_rows.get(key), b_rows.get(key), key_field))
        .collect();
    Ok(json!({
        "metric": describe(&metric),
        "statistic": Value::from(key_field),
        "period_a": window_json(first),
        "period_b": window_json(second),
        "rows": Value::Array(rows),
        "notes": Value::Array(vec![Value::from(CAUSATION_NOTE)]),
    }))
}

/// The values of one group key: a NULL dimension is `None`.
type GroupKey = Vec<Option<String>>;

/// The rows keyed by their group values. A later row with the same key replaces an earlier one, as
/// the Python dict does.
fn by_key(rows: &[Value], group: &[String]) -> RedBlackTreeMapSync<GroupKey, Value> {
    rows.iter()
        .fold(RedBlackTreeMapSync::new_sync(), |index, row| {
            let key: GroupKey = group
                .iter()
                .map(|dimension| {
                    row.get(dimension)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            index.insert(key, row.clone())
        })
}

/// One compare row: the group values, `a`, `b`, `delta`, `relative_change`, `n_a` and `n_b`.
fn comparison_row(
    key: &GroupKey,
    group: &[String],
    a: Option<&Value>,
    b: Option<&Value>,
    key_field: &str,
) -> Value {
    let statistic = |row: Option<&Value>| {
        row.and_then(|row| row.get(key_field))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let count = |row: Option<&Value>| {
        row.and_then(|row| row.get("n"))
            .cloned()
            .unwrap_or(Value::from(0))
    };
    let (va, vb) = (statistic(a), statistic(b));
    let delta = difference(&va, &vb);
    let relative = delta.as_ref().and_then(|delta| relative_change(delta, &va));
    let dimensions = group.iter().zip(key).map(|(dimension, value)| {
        (
            dimension.clone(),
            value.clone().map_or(Value::Null, Value::String),
        )
    });
    let statistics = [
        ("a", va.clone()),
        ("b", vb.clone()),
        ("delta", delta.unwrap_or(Value::Null)),
        ("relative_change", relative.unwrap_or(Value::Null)),
        ("n_a", count(a)),
        ("n_b", count(b)),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value));
    Value::Object(dimensions.chain(statistics).collect())
}

/// `b - a` of two JSON numbers: an integer when both are integers, else a float. NULL otherwise.
fn difference(a: &Value, b: &Value) -> Option<Value> {
    if !a.is_number() || !b.is_number() {
        return None;
    }
    match (a.as_i64(), b.as_i64()) {
        (Some(x), Some(y)) => Some(Value::from(y - x)),
        _ => Some(Value::from(b.as_f64()? - a.as_f64()?)),
    }
}

/// `delta / a` when `a` is a non-zero number (Python's truthiness), as a float.
fn relative_change(delta: &Value, a: &Value) -> Option<Value> {
    let divisor = a.as_f64().filter(|value| *value != 0.0)?;
    Some(Value::from(delta.as_f64()? / divisor))
}

/// `repr` of a key tuple of strings and NULLs, which is what `str(tuple)` gives in Python.
fn tuple_repr(key: &GroupKey) -> String {
    let items: Vec<String> = key
        .iter()
        .map(|value| {
            value
                .as_deref()
                .map_or_else(|| "None".to_string(), str_repr)
        })
        .collect();
    match items.as_slice() {
        [single] => format!("({single},)"),
        _ => format!("({})", items.join(", ")),
    }
}

/// `repr` of a Python `str`: single quotes unless the text holds a single quote and no double
/// quote, with backslash, the quote, and the control characters escaped.
fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c == quote => format!("\\{c}"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("\\x{:02x}", c as u32),
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

/// A window as the two ISO instants of the Python `isoformat()`.
fn window_json((since, until): Window) -> Value {
    Value::Array(vec![
        Value::from(at_offset(since)),
        Value::from(at_offset(until)),
    ])
}

/// An aware UTC instant as `isoformat()` writes it: `...+00:00`.
fn at_offset(at: Timestamp) -> String {
    at.format_isoformat()
}

/// A naive (UTC) instant as `isoformat()` writes it: the warehouse stores naive timestamps.
fn naive_iso(at: Timestamp) -> String {
    let text = at.format_isoformat();
    text.strip_suffix("+00:00")
        .map_or(text.clone(), str::to_string)
}

/// `task_history`: the accepted tasks of a workspace, newest acceptance first, at most `limit` of
/// them, with their measured characteristics. The optional category and repository narrow them.
pub fn task_history(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    category: Option<&str>,
    repository: Option<&str>,
    limit: usize,
) -> Value {
    let linked = |task: &TaskFact| -> bool {
        repository.is_none_or(|wanted| {
            task.task.id.is_some()
                && input.task_repositories.iter().any(|link| {
                    link.task_id == task.task.id && link.repository_id.as_deref() == Some(wanted)
                })
        })
    };
    let accepted = facts
        .task_facts
        .iter()
        .filter(|fact| fact.task.workspace_id.as_deref() == Some(workspace))
        .filter(|fact| category.is_none_or(|wanted| fact.task.category.as_deref() == Some(wanted)))
        .filter(|fact| linked(fact))
        .filter_map(|fact| fact.task.accepted_at.map(|at| (at, fact)));
    // Newest acceptance first; ties keep the order of the task table.
    let ordered = accepted.enumerate().fold(
        RedBlackTreeMapSync::<(Reverse<Timestamp>, usize), &TaskFact>::new_sync(),
        |ordered, (position, (at, fact))| ordered.insert((Reverse(at), position), fact),
    );
    Value::Array(
        ordered
            .values()
            .take(limit)
            .map(|fact| history_row(fact))
            .collect(),
    )
}

/// One row of [`task_history`]: the columns of the Python `SELECT` in the order it lists them.
fn history_row(fact: &TaskFact) -> Value {
    let task = &fact.task;
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let number = |value: i64| Value::from(value);
    let optional = |value: Option<i64>| value.map_or(Value::Null, Value::from);
    let accepted_at = task
        .accepted_at
        .map_or(Value::Null, |at| Value::from(naive_iso(at)));
    json!({
        "key": text(&task.key),
        "title": text(&task.title),
        "category": text(&task.category),
        "status": text(&task.status),
        "attempts": number(fact.attempts),
        "retries": number(fact.retries),
        "execution_ms": optional(fact.execution_ms),
        "queue_wait_ms": optional(fact.queue_wait_ms),
        "elapsed_ms": optional(fact.elapsed_ms),
        "commits": number(fact.commits),
        "churn_insertions": number(fact.churn_insertions),
        "churn_deletions": number(fact.churn_deletions),
        "final_insertions": optional(fact.final_insertions),
        "final_deletions": optional(fact.final_deletions),
        "files_touched": number(fact.files_touched),
        "repositories_touched": number(fact.repositories_touched),
        "reopen_count": number(fact.reopen_count),
        "accepted_at": accepted_at,
    })
}
