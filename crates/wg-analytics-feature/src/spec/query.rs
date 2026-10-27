//! Pure: the `analytics metrics` and `analytics query` decisions (`analytics/service.py`
//! `definitions` and `query`): the request, its validation, the query it runs and the result
//! document. The metric engine is `wg-analytics`; the store reads are in [`crate::service`].
//!
//! The provenance `sql` is the DuckDB text of the Python service. The Rust engine does not
//! produce that text (divergence DV3), so the field carries a description of the engine instead.

use std::collections::BTreeSet;

use serde_json::Value as Json;
use wg_analytics::Facts;
use wg_analytics::core::metrics::{Metric, describe, find, registry};
use wg_analytics::core::period::Period;
use wg_analytics::core::query::{Answer, Filters, Query, QueryError, run};
use wg_analytics::core::service::CAUSATION_NOTE;
use wg_analytics::source::AnalyticsInput;
use wg_json::{LocalDateTime, PyStr, Value};
use wg_types::{ErrorCode, Timestamp, WgError};

/// The `source` of the provenance block (`AnalyticsService.query`).
pub const SOURCE: &str = "DuckDB snapshot of the SQLite database";

/// What the provenance `sql` holds instead of DuckDB text.
pub const SQL_DESCRIPTION: &str =
    "wg-analytics engine: fact tables and metric registry (DuckDB SQL text not reproduced, DV3)";

/// The row limit of an `AnalyticsQuery` (its default).
pub const LIMIT: usize = 500;

/// The `analytics query` options, as the command reads them. Dates are naive (typer's `datetime`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryRequest {
    pub metric: String,
    pub group_by: Vec<String>,
    pub period: String,
    pub project: Option<String>,
    pub repository: Option<String>,
    pub category: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub since: Option<LocalDateTime>,
    pub until: Option<LocalDateTime>,
}

/// A JSON object from its entries, in the order given.
fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}

fn text(value: &str) -> Value {
    Value::Str(PyStr::from(value))
}

fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

/// A `serde_json` value as a Python-shaped value (object key order kept).
#[must_use]
pub fn json_value(value: &Json) -> Value {
    match value {
        Json::Null => Value::Null,
        Json::Bool(flag) => Value::Bool(*flag),
        Json::Number(number) => number.as_i64().map_or_else(
            || Value::Float(number.as_f64().unwrap_or(f64::NAN)),
            |int| Value::Int(i128::from(int)),
        ),
        Json::String(string) => text(string),
        Json::Array(items) => Value::List(items.iter().map(json_value).collect()),
        Json::Object(map) => Value::Dict(
            map.iter()
                .map(|(key, value)| (PyStr::from(key.as_str()), json_value(value)))
                .collect(),
        ),
    }
}

/// The metric registry as `metric_definition` dicts, in registry order (`AnalyticsService
/// .definitions`).
#[must_use]
pub fn definitions() -> Value {
    Value::List(
        registry()
            .iter()
            .map(|metric| json_value(&describe(metric)))
            .collect(),
    )
}

/// The registry's metric ids, sorted (the list of Python's `known metrics` message).
fn known_metrics() -> String {
    let ids: BTreeSet<&str> = registry().iter().map(|metric| metric.id).collect();
    ids.into_iter().collect::<Vec<_>>().join(", ")
}

/// The pydantic checks of `AnalyticsQuery` (metric name, period literal), which the CLI runs
/// before any store read. Returns the metric.
pub fn check_request(request: &QueryRequest) -> Result<Metric, WgError> {
    let metric = find(&request.metric).ok_or_else(|| {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!(
                "unknown metric {:?}; known metrics: {}",
                request.metric,
                known_metrics()
            ),
        )
    })?;
    if Period::parse(&request.period).is_none() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!(
                "period: input should be 'day', 'week', 'month', 'quarter' or 'year' ({:?})",
                request.period
            ),
        ));
    }
    Ok(metric)
}

/// A naive typer datetime as the UTC instant the warehouse compares it with.
fn instant(at: LocalDateTime) -> Result<Timestamp, WgError> {
    Timestamp::from_components(
        at.year,
        at.month,
        at.day,
        at.hour,
        at.minute,
        at.second,
        at.microsecond,
    )
}

/// The engine's query for a request, with the project and repository already resolved to ids.
pub fn engine_query(
    request: &QueryRequest,
    project_id: Option<String>,
    repository_id: Option<String>,
) -> Result<Query, WgError> {
    let period = Period::parse(&request.period).ok_or_else(|| {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!("unknown period {:?}", request.period),
        )
    })?;
    Ok(Query {
        metric: request.metric.clone(),
        group_by: request.group_by.clone(),
        period,
        filters: Filters {
            project: project_id,
            repository: repository_id,
            category: request.category.clone(),
            agent: None,
            provider: request.provider.clone(),
            model: request.model.clone(),
            priority: None,
            status: None,
            kind: None,
            since: request.since.map(instant).transpose()?,
            until: request.until.map(instant).transpose()?,
        },
        limit: LIMIT,
    })
}

/// Runs the engine. A refused query (dimension, filter) is a validation failure.
pub fn run_query(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    query: &Query,
) -> Result<Answer, WgError> {
    run(input, facts, workspace, query)
        .map_err(|err: QueryError| WgError::new(ErrorCode::ValidationFailed, err.to_string()))
}

/// The `query` echo: `AnalyticsQuery.model_dump(mode="json")`, with the dates as they were typed.
fn echo(request: &QueryRequest) -> Value {
    let date = |at: Option<LocalDateTime>| at.map_or(Value::Null, Value::LocalDateTime);
    dict(vec![
        ("metric", text(&request.metric)),
        (
            "group_by",
            Value::List(request.group_by.iter().map(|dim| text(dim)).collect()),
        ),
        ("period", text(&request.period)),
        (
            "filters",
            dict(vec![
                ("project", optional_text(request.project.as_deref())),
                ("repository", optional_text(request.repository.as_deref())),
                ("category", optional_text(request.category.as_deref())),
                ("agent", Value::Null),
                ("provider", optional_text(request.provider.as_deref())),
                ("model", optional_text(request.model.as_deref())),
                ("priority", Value::Null),
                ("status", Value::Null),
                ("kind", Value::Null),
                ("since", date(request.since)),
                ("until", date(request.until)),
            ]),
        ),
        ("limit", Value::Int(LIMIT as i128)),
    ])
}

/// The `query` result document (`AnalyticsQueryOut`), in the Python field order.
#[must_use]
pub fn query_out(
    request: &QueryRequest,
    metric: &Metric,
    answer: &Answer,
    generated_at: Timestamp,
) -> Value {
    let rows = answer.rows.iter().map(json_value).collect();
    let notes: Vec<Value> = metric
        .caveats
        .iter()
        .copied()
        .chain(std::iter::once(CAUSATION_NOTE))
        .map(text)
        .collect();
    dict(vec![
        ("metric", json_value(&describe(metric))),
        ("query", echo(request)),
        ("rows", Value::List(rows)),
        ("sample_size", Value::Int(i128::from(answer.sample_size))),
        (
            "provenance",
            dict(vec![
                ("sql", text(SQL_DESCRIPTION)),
                ("source", text(SOURCE)),
                ("generated_at", text(&generated_at.format_isoformat())),
            ]),
        ),
        ("notes", Value::List(notes)),
    ])
}
