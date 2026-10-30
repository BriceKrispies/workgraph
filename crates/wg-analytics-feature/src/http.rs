//! The analytics routes (`api/routes/ops.py`, analytics): the metric catalog (H101), the query
//! (H102), the overview (H103), the compare (H104), the coupling (H105), the failures (H106), the
//! agent time (H107), the bottlenecks (H108), the history (H109), the estimate (H110) and the
//! retrospective (H113), under `/api/v1`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points. The export (H111)
//! and the stored reports (H112) are not served yet: the export writes DuckDB files and the report
//! list reads the stored `report.generated` events, neither of which is ported.

use serde_json::{Map, Value as Json};
use wg_clispec::datetime::parse_click_datetime;
use wg_json::{LocalDateTime, Value};
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::WgError;

use crate::service;
use crate::spec::analyses::Typed;
use crate::spec::query::{self, QueryRequest};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 11] = [
    RouteSpec {
        id: "H101",
        method: "GET",
        path: "/api/v1/analytics/metrics",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H102",
        method: "POST",
        path: "/api/v1/ws/{ws}/analytics/query",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "AnalyticsQuery",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H103",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/overview",
        status: 200,
        query: &["period"],
        body: None,
    },
    RouteSpec {
        id: "H104",
        method: "POST",
        path: "/api/v1/ws/{ws}/analytics/compare",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "CompareBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H105",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/coupling",
        status: 200,
        query: &["project", "min_support"],
        body: None,
    },
    RouteSpec {
        id: "H106",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/failures",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H107",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/agent-time",
        status: 200,
        query: &["since", "until", "by", "model"],
        body: None,
    },
    RouteSpec {
        id: "H108",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/bottlenecks",
        status: 200,
        query: &["since", "until", "top"],
        body: None,
    },
    RouteSpec {
        id: "H109",
        method: "GET",
        path: "/api/v1/ws/{ws}/analytics/history",
        status: 200,
        query: &["category", "repository", "limit"],
        body: None,
    },
    RouteSpec {
        id: "H110",
        method: "POST",
        path: "/api/v1/ws/{ws}/analytics/estimate",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "EstimateBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H113",
        method: "POST",
        path: "/api/v1/ws/{ws}/reports/retrospective",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "RetroBody",
            strip: false,
            required: true,
        }),
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params,
        body,
        context,
        ..
    } = request;
    let ws = params.get("ws").and_then(Json::as_str);
    let context = &context;
    match id {
        "H101" => Ok(query::definitions()),
        "H102" => service::query(app, context, ws, &query_body(&body)?),
        "H103" => service::overview(app, context, ws, text(&params, "period").unwrap_or("week")),
        "H104" => compare_body(app, context, ws, &body),
        "H105" => service::coupling(
            app,
            context,
            ws,
            text(&params, "project"),
            number(&params, "min_support", 1),
        ),
        "H106" => service::failures(app, context, ws, false),
        "H107" => service::subagents::breakdown(
            app,
            context,
            ws,
            when(&params, "since")?,
            when(&params, "until")?,
            text(&params, "by").unwrap_or("class"),
            text(&params, "model"),
        ),
        "H108" => {
            let top = number(&params, "top", 15);
            service::subagents::bottlenecks(
                app,
                context,
                ws,
                when(&params, "since")?,
                when(&params, "until")?,
                usize::try_from(top.max(0)).unwrap_or(usize::MAX),
            )
        }
        "H109" => service::history(
            app,
            context,
            ws,
            text(&params, "category"),
            text(&params, "repository"),
            number(&params, "limit", 50),
        ),
        "H110" => estimate_body(app, context, ws, &body),
        "H113" => {
            let options = (
                body.get("period_days").and_then(Json::as_i64).unwrap_or(14),
                text_json(&body, "project").map(str::to_owned),
                text_json(&body, "repository").map(str::to_owned),
                body.get("use_model")
                    .and_then(Json::as_bool)
                    .unwrap_or(true),
                None,
            );
            service::retrospective::retrospective(app, context, ws, &options)
        }
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

/// A query or path parameter as text, when the request carries it.
fn text<'a>(params: &'a Map<String, Json>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Json::as_str)
}

/// An integer parameter, read from a number or from its text; `default` when absent.
fn number(params: &Map<String, Json>, key: &str, default: i64) -> i64 {
    params
        .get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(default)
}

/// A body field as text, when it is a string.
fn text_json<'a>(body: &'a Json, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Json::as_str)
}

/// A date-time parameter (`since`, `until`): Click's datetime formats, absent when not given.
fn when(params: &Map<String, Json>, key: &str) -> Result<Option<LocalDateTime>, WgError> {
    text(params, key).map(instant).transpose()
}

/// A date-time as the request gives it, read in the same formats the CLI reads.
fn instant(text: &str) -> Result<LocalDateTime, WgError> {
    parse_click_datetime(text)
        .map_err(|message| WgError::validation(format!("invalid datetime '{text}': {message}")))
}

/// A list of strings from a body field (absent or not a list: empty).
fn strings(value: Option<&Json>) -> Vec<String> {
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

/// The `AnalyticsQuery` body as the query request: `metric`, `group_by`, `period` and the
/// `filters` (their `since` and `until` are the window).
fn query_body(body: &Json) -> Result<QueryRequest, WgError> {
    let filters = body.get("filters").cloned().unwrap_or(Json::Null);
    let optional = |key: &str| text_json(&filters, key).map(str::to_owned);
    Ok(QueryRequest {
        metric: text_json(body, "metric").unwrap_or_default().to_owned(),
        group_by: strings(body.get("group_by")),
        period: text_json(body, "period").unwrap_or("week").to_owned(),
        project: optional("project"),
        repository: optional("repository"),
        category: optional("category"),
        provider: optional("provider"),
        model: optional("model"),
        since: text_json(&filters, "since").map(instant).transpose()?,
        until: text_json(&filters, "until").map(instant).transpose()?,
    })
}

/// The `CompareBody` body: the metric, the two windows and the grouping.
fn compare_body(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    let window = |since: &str, until: &str| -> Result<Typed, WgError> {
        Ok((
            instant(text_json(body, since).unwrap_or_default())?,
            instant(text_json(body, until).unwrap_or_default())?,
        ))
    };
    let first = window("a_since", "a_until")?;
    let second = window("b_since", "b_until")?;
    service::compare(
        app,
        context,
        ws,
        text_json(body, "metric").unwrap_or_default(),
        first,
        second,
        &strings(body.get("group_by")),
    )
}

/// The `EstimateBody` body: the task or the title, the category and the repositories.
fn estimate_body(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    service::estimate::estimate(
        app,
        context,
        ws,
        text_json(body, "task"),
        text_json(body, "title"),
        text_json(body, "category"),
        &strings(body.get("repositories")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_table_lists_the_served_operations_in_order() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H101", "H102", "H103", "H104", "H105", "H106", "H107", "H108", "H109", "H110",
                "H113"
            ]
        );
    }
}
