//! The intelligence routes (`api/routes/ops.py`, search and classification reads): the search
//! (H114), the classification evaluations (H115) and the disagreements (H116), under `/api/v1`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points. The search's
//! `created_after` and `created_before` filters are not ported yet: the service's filters do not
//! carry them, so a request that gives either one is refused rather than answered without it.

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_serverspec::groups::RouteSpec;
use wg_types::WgError;

use crate::service::entry;
use crate::service::llm::Live;
use crate::spec::requests::SearchRequest;
use crate::spec::search_out::SearchFilters;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 3] = [
    RouteSpec {
        id: "H114",
        method: "GET",
        path: "/api/v1/ws/{ws}/search",
        status: 200,
        query: &[
            "q",
            "mode",
            "project",
            "repository",
            "category",
            "status",
            "agent",
            "model",
            "path",
            "created_after",
            "created_before",
            "limit",
        ],
        body: None,
    },
    RouteSpec {
        id: "H115",
        method: "GET",
        path: "/api/v1/ws/{ws}/classification/evaluations",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H116",
        method: "GET",
        path: "/api/v1/ws/{ws}/classification/disagreements",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params, context, ..
    } = request;
    let ws = params.get("ws").and_then(Json::as_str);
    match id {
        "H114" => {
            if params.contains_key("created_after") || params.contains_key("created_before") {
                return Err(WgError::workgraph(
                    "the created_after and created_before search filters are not ported yet",
                ));
            }
            let request = search_request(&params);
            entry::search(app, &context, &mut Live, ws, &request)
        }
        "H115" => entry::classify_evaluations(app, &context, ws),
        "H116" => entry::classify_disagreements(app, &context, ws),
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

/// A query parameter as text, when the request carries it.
fn text<'a>(params: &'a Map<String, Json>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Json::as_str)
}

/// The search arguments: `q`, `mode` (default `hybrid`), `limit` (default 20) and the filters.
/// `status` may repeat, so it is a list.
fn search_request(params: &Map<String, Json>) -> SearchRequest {
    let statuses = match params.get("status") {
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Json::String(one)) => vec![one.clone()],
        _ => Vec::new(),
    };
    let limit = params
        .get("limit")
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(20);
    SearchRequest {
        query: text(params, "q").unwrap_or_default().to_owned(),
        mode: text(params, "mode").unwrap_or("hybrid").to_owned(),
        limit,
        filters: SearchFilters {
            project: text(params, "project").map(str::to_owned),
            repository: text(params, "repository").map(str::to_owned),
            category: text(params, "category").map(str::to_owned),
            statuses,
            agent: text(params, "agent").map(str::to_owned),
            model: text(params, "model").map(str::to_owned),
            changed_path: text(params, "path").map(str::to_owned),
        },
    }
}
