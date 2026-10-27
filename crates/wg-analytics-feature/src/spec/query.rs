//! Pure: the `analytics metrics` and `analytics query` decisions (`analytics/service.py`
//! `definitions` and `query`): the request, its validation, the query it runs and the result
//! document. The metric engine is `wg-analytics`; the store reads are in [`crate::service`].
//!
//! The provenance `sql` is the DuckDB text of the Python service. The Rust engine does not
//! produce that text (divergence DV3), so the field carries a description of the engine instead.

use std::collections::BTreeSet;

use serde_json::{Map, Value as Json, json};
use wg_analytics::Facts;
use wg_analytics::core::bases::dimensions;
use wg_analytics::core::metrics::{Metric, describe, find, registry};
use wg_analytics::core::period::Period;
use wg_analytics::core::query::{Answer, Filters, Query, QueryError, run};
use wg_analytics::core::service::CAUSATION_NOTE;
use wg_analytics::source::AnalyticsInput;
use wg_json::{LocalDateTime, Value, dict};
use wg_tasks::requests::validation_failure;
use wg_types::{Timestamp, WgError};
use wg_validate::{Loc, ValidationError};
use wg_wire::convention::{json_value, optional_text, text};

/// The `source` of the provenance block (`AnalyticsService.query`).
pub const SOURCE: &str = "DuckDB snapshot of the SQLite database";

/// What the provenance `sql` holds instead of DuckDB text.
pub const SQL_DESCRIPTION: &str =
    "wg-analytics engine: fact tables and metric registry (DuckDB SQL text not reproduced, DV3)";

/// The row limit of an `AnalyticsQuery` (its default).
pub const LIMIT: usize = 500;

/// What pydantic lists as the expected values of the `period` literal.
const PERIOD_CHOICES: &str = "'day', 'week', 'month', 'quarter' or 'year'";

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

/// The pydantic error of the `metric` validator: `ValueError` text, which `--json` prints as the
/// `ctx.error` string (`json.dumps(default=str)`).
fn unknown_metric(metric: &str) -> ValidationError {
    let reason = format!(
        "unknown metric {}; known metrics: {}",
        wg_types::py_repr_str(metric),
        known_metrics()
    );
    ValidationError {
        kind: "value_error".to_owned(),
        loc: vec![Loc::Key("metric".to_owned())],
        msg: format!("Value error, {reason}"),
        input: json!(metric),
        ctx: Some(Map::from_iter([("error".to_owned(), json!(reason))])),
    }
}

/// The pydantic error of the `period` literal.
fn bad_period(period: &str) -> ValidationError {
    ValidationError {
        kind: "literal_error".to_owned(),
        loc: vec![Loc::Key("period".to_owned())],
        msg: format!("Input should be {PERIOD_CHOICES}"),
        input: json!(period),
        ctx: Some(Map::from_iter([(
            "expected".to_owned(),
            json!(PERIOD_CHOICES),
        )])),
    }
}

/// The pydantic checks of `AnalyticsQuery` (metric name, period literal), which every command runs
/// before any permission or store read. Both fields are reported when both are wrong, `metric`
/// first, as `handle_errors` reports a `ValidationError`. Returns the metric.
pub fn check_inputs(metric: &str, period: &str) -> Result<Metric, WgError> {
    let found = find(metric);
    let errors: Vec<ValidationError> = [
        found.is_none().then(|| unknown_metric(metric)),
        Period::parse(period).is_none().then(|| bad_period(period)),
    ]
    .into_iter()
    .flatten()
    .collect();
    match found {
        Some(known) if errors.is_empty() => Ok(known),
        _ => Err(validation_failure(&errors)),
    }
}

/// [`check_inputs`] for an `analytics query` request.
pub fn check_request(request: &QueryRequest) -> Result<Metric, WgError> {
    check_inputs(&request.metric, &request.period)
}

/// A naive typer datetime as the UTC instant the warehouse compares it with.
pub fn instant(at: LocalDateTime) -> Result<Timestamp, WgError> {
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
        WgError::validation(format!(
            "unknown period {}",
            wg_types::py_repr_str(&request.period)
        ))
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

/// A refused query as `ValidationFailed`: a dimension the metric cannot be grouped by lists the
/// ones it can (`details.allowed`, sorted, with `period`).
#[must_use]
pub fn refusal(err: &QueryError) -> WgError {
    let refused = WgError::validation(err.to_string());
    match err {
        QueryError::Dimension { metric, .. } => find(metric).map_or(refused.clone(), |known| {
            let allowed: BTreeSet<&str> = dimensions(known.base)
                .iter()
                .copied()
                .chain(std::iter::once("period"))
                .collect();
            refused.detail(
                "allowed",
                Json::Array(allowed.into_iter().map(Json::from).collect()),
            )
        }),
        _ => refused,
    }
}

/// Runs the engine. A refused query (dimension, filter) is a validation failure.
pub fn run_query(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    query: &Query,
) -> Result<Answer, WgError> {
    run(input, facts, workspace, query).map_err(|err| refusal(&err))
}

/// The `query` echo: `AnalyticsQuery.model_dump(mode="json")`, with the dates as they were typed.
fn echo(request: &QueryRequest) -> Value {
    let date = |at: Option<LocalDateTime>| at.map_or(Value::Null, Value::LocalDateTime);
    dict!({
        "metric": text(&request.metric),
        "group_by": Value::List(request.group_by.iter().map(|dim| text(dim)).collect()),
        "period": text(&request.period),
        "filters": dict!({
            "project": optional_text(request.project.as_deref()),
            "repository": optional_text(request.repository.as_deref()),
            "category": optional_text(request.category.as_deref()),
            "agent": Value::Null,
            "provider": optional_text(request.provider.as_deref()),
            "model": optional_text(request.model.as_deref()),
            "priority": Value::Null,
            "status": Value::Null,
            "kind": Value::Null,
            "since": date(request.since),
            "until": date(request.until),
        }),
        "limit": Value::Int(LIMIT as i128),
    })
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
    dict!({
        "metric": json_value(&describe(metric)),
        "query": echo(request),
        "rows": Value::List(rows),
        "sample_size": Value::Int(i128::from(answer.sample_size)),
        "provenance": dict!({
            "sql": text(SQL_DESCRIPTION),
            "source": text(SOURCE),
            "generated_at": text(&generated_at.format_isoformat()),
        }),
        "notes": Value::List(notes),
    })
}
