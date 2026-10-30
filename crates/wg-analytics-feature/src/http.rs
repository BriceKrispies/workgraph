//! The analytics routes (`api/routes/ops.py`, analytics): the metric catalog (H101), the query
//! (H102), the overview (H103), the compare (H104), the coupling (H105), the failures (H106), the
//! agent time (H107), the bottlenecks (H108), the history (H109), the estimate (H110), the stored
//! reports (H112) and the retrospective (H113), under `/api/v1`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points. The export (H111)
//! is not served: its Python route answers with the written file, and a feature route can only
//! answer JSON.

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::WgError;

use crate::service;
use crate::spec::analyses::Typed;
use crate::spec::params::{
    body_text, instant_text, json_strings, param_instant, param_number, param_text, query_body,
};
use crate::spec::query;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 12] = [
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
        id: "H112",
        method: "GET",
        path: "/api/v1/ws/{ws}/reports",
        status: 200,
        query: &[],
        body: None,
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
        "H103" => service::overview(
            app,
            context,
            ws,
            param_text(&params, "period").unwrap_or("week"),
        ),
        "H104" => compare_body(app, context, ws, &body),
        "H105" => service::coupling(
            app,
            context,
            ws,
            param_text(&params, "project"),
            param_number(&params, "min_support", 1),
        ),
        "H106" => service::failures(app, context, ws, false),
        "H107" => service::subagents::breakdown(
            app,
            context,
            ws,
            param_instant(&params, "since")?,
            param_instant(&params, "until")?,
            param_text(&params, "by").unwrap_or("class"),
            param_text(&params, "model"),
        ),
        "H108" => {
            let top = param_number(&params, "top", 15);
            service::subagents::bottlenecks(
                app,
                context,
                ws,
                param_instant(&params, "since")?,
                param_instant(&params, "until")?,
                usize::try_from(top.max(0)).unwrap_or(usize::MAX),
            )
        }
        "H109" => service::history(
            app,
            context,
            ws,
            param_text(&params, "category"),
            param_text(&params, "repository"),
            param_number(&params, "limit", 50),
        ),
        "H110" => estimate_body(app, context, ws, &body),
        "H112" => service::reports::list_reports(app, context, ws),
        "H113" => {
            let options = (
                body.get("period_days").and_then(Json::as_i64).unwrap_or(14),
                body_text(&body, "project").map(str::to_owned),
                body_text(&body, "repository").map(str::to_owned),
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

/// The `CompareBody` body: the metric, the two windows and the grouping.
fn compare_body(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    let window = |since: &str, until: &str| -> Result<Typed, WgError> {
        Ok((
            instant_text(body_text(body, since).unwrap_or_default())?,
            instant_text(body_text(body, until).unwrap_or_default())?,
        ))
    };
    let first = window("a_since", "a_until")?;
    let second = window("b_since", "b_until")?;
    service::compare(
        app,
        context,
        ws,
        body_text(body, "metric").unwrap_or_default(),
        first,
        second,
        &json_strings(body.get("group_by")),
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
        body_text(body, "task"),
        body_text(body, "title"),
        body_text(body, "description").unwrap_or_default(),
        body_text(body, "category"),
        &json_strings(body.get("repositories")),
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
                "H112", "H113"
            ]
        );
    }
}
