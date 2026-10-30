//! The intelligence routes: the task's classification (H052 to H054, `api/routes/tasks.py`) and
//! its similar tasks (H055), the search (H114), the classification evaluations (H115) and the
//! disagreements (H116), under `/api/v1` (`api/routes/ops.py` for the last three).
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points. The search's
//! `created_after` and `created_before` filters are read as UTC instants.

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::WgError;

use crate::service::entry;
use crate::service::llm::Live;
use crate::spec::requests::{ConfirmRequest, RunOptions, SearchRequest, instant_param};
use crate::spec::search_out::SearchFilters;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 7] = [
    RouteSpec {
        id: "H052",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/classification",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H053",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/classification",
        status: 200,
        query: &["provider", "model", "promote"],
        body: None,
    },
    RouteSpec {
        id: "H054",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/classification/confirm",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "ConfirmBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H055",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/similar",
        status: 200,
        query: &["limit"],
        body: None,
    },
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
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: |app, id, request| run(app, id, request).map(RouteAnswer::Json),
};

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params,
        body,
        context,
        ..
    } = request;
    let ws = params.get("ws").and_then(Json::as_str);
    match id {
        "H052" => entry::classify_show(app, &context, ws, text(&params, "ref").unwrap_or_default()),
        "H053" => {
            let options = RunOptions {
                provider: text(&params, "provider").map(str::to_owned),
                model: text(&params, "model").map(str::to_owned),
                promote: flag(&params, "promote"),
                batch_id: None,
            };
            entry::classify_request(
                app,
                &context,
                ws,
                text(&params, "ref").unwrap_or_default(),
                &options,
            )
        }
        "H054" => {
            let request = ConfirmRequest {
                category: body_text(&body, "category").unwrap_or_default().to_owned(),
                subtype: None,
                domain: None,
                complexity: body_text(&body, "complexity").map(str::to_owned),
                tags: Vec::new(),
                note: body_text(&body, "note").unwrap_or_default().to_owned(),
                add_to_gold: true,
            };
            entry::classify_confirm(
                app,
                &context,
                ws,
                text(&params, "ref").unwrap_or_default(),
                &request,
            )
        }
        "H055" => {
            let limit = params
                .get("limit")
                .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                .unwrap_or(10);
            let limit = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
            entry::search_similar(
                app,
                &context,
                ws,
                text(&params, "ref").unwrap_or_default(),
                limit,
            )
        }
        "H114" => {
            let request = search_request(&params)?;
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

/// A boolean query parameter: `false` when absent, read from a JSON boolean or from its text.
fn flag(params: &Map<String, Json>, key: &str) -> bool {
    match params.get(key) {
        Some(Json::Bool(value)) => *value,
        Some(Json::String(text)) => matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        ),
        _ => false,
    }
}

/// A string member of the body; `None` when the body is absent or the member is not a string.
fn body_text<'a>(body: &'a Json, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Json::as_str)
}

/// The search arguments: `q`, `mode` (default `hybrid`), `limit` (default 20) and the filters.
/// `status` may repeat, so it is a list.
fn search_request(params: &Map<String, Json>) -> Result<SearchRequest, WgError> {
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
    Ok(SearchRequest {
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
            created_after: instant_param(params, "created_after")?,
            created_before: instant_param(params, "created_before")?,
        },
    })
}
