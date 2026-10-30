//! Pure: the request readers the HTTP routes, the MCP tools and the subagent reports share. Each
//! reads a JSON document it is given (a route's parameters or body, a tool's arguments) and
//! returns the typed value the service takes; none of them touches the store or the runtime.

use serde_json::{Map, Value as Json};
use wg_analytics::agent_time::Group;
use wg_analytics::common::Window;
use wg_clispec::datetime::parse_click_datetime;
use wg_json::LocalDateTime;
use wg_mcpspec::text_argument;
use wg_types::WgError;

use super::query::{self, QueryRequest};

/// A query or path parameter as text, when the request carries it.
pub fn param_text<'a>(params: &'a Map<String, Json>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Json::as_str)
}

/// An integer parameter, read from a number or from its text; `default` when absent.
pub fn param_number(params: &Map<String, Json>, key: &str, default: i64) -> i64 {
    params
        .get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(default)
}

/// A date-time parameter (`since`, `until`): Click's datetime formats, absent when not given.
pub fn param_instant(
    params: &Map<String, Json>,
    key: &str,
) -> Result<Option<LocalDateTime>, WgError> {
    param_text(params, key).map(instant_text).transpose()
}

/// A body field as text, when it is a string.
pub fn body_text<'a>(body: &'a Json, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Json::as_str)
}

/// A list of strings from a JSON field (absent or not a list: empty).
pub fn json_strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A date-time as the request gives it, read in the same formats the CLI reads.
pub fn instant_text(text: &str) -> Result<LocalDateTime, WgError> {
    parse_click_datetime(text)
        .map_err(|message| WgError::validation(format!("invalid datetime '{text}': {message}")))
}

/// A date argument of an MCP tool (`since` or `until`) in the formats the CLI reads, absent when
/// not given.
pub fn argument_date(arguments: &Json, key: &str) -> Result<Option<LocalDateTime>, WgError> {
    text_argument(arguments, key)
        .map(|text| {
            parse_click_datetime(text)
                .map_err(|message| WgError::validation(format!("invalid date: {message}")))
        })
        .transpose()
}

/// The `AnalyticsQuery` body as the query request: `metric`, `group_by`, `period` and the
/// `filters` (their `since` and `until` are the window).
pub fn query_body(body: &Json) -> Result<QueryRequest, WgError> {
    let filters = body.get("filters").cloned().unwrap_or(Json::Null);
    let optional = |key: &str| body_text(&filters, key).map(str::to_owned);
    Ok(QueryRequest {
        metric: body_text(body, "metric").unwrap_or_default().to_owned(),
        group_by: json_strings(body.get("group_by")),
        period: body_text(body, "period").unwrap_or("week").to_owned(),
        project: optional("project"),
        repository: optional("repository"),
        category: optional("category"),
        provider: optional("provider"),
        model: optional("model"),
        since: body_text(&filters, "since").map(instant_text).transpose()?,
        until: body_text(&filters, "until").map(instant_text).transpose()?,
    })
}

/// The window of a report: the inclusive `since` and exclusive `until`, and the model filter.
pub fn report_window(
    since: Option<LocalDateTime>,
    until: Option<LocalDateTime>,
    model: Option<&str>,
) -> Result<Window, WgError> {
    Ok(Window {
        since: since.map(query::instant).transpose()?,
        until: until.map(query::instant).transpose()?,
        model: model.map(str::to_owned),
    })
}

/// The grouping names `analytics time --by` accepts.
pub fn time_group(by: &str) -> Result<Group, WgError> {
    Group::parse(by).ok_or_else(|| {
        WgError::validation(format!("unknown grouping '{by}'")).detail(
            "allowed",
            serde_json::json!(["class", "model", "task", "description"]),
        )
    })
}
