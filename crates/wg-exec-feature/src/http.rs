//! The exec feature's routes (docs/rust-rewrite.md 4.16, IF-46): the attempt reads (H071, H073 to
//! H076), the run routes (H072, H074, H077 to H079) and the scheduler overview (H122).
//!
//! [`SPECS`] is the table of operations; [`GROUP`] binds each one to [`run_exec`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points. The permission
//! checks are made first, as the Python routes make them; the run routes check theirs inside the
//! run flow (`RunService`), which runs before any reference is resolved, as Python does.

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_types::{Permission, WgError};

use crate::service::agents::LiveAgents;
use crate::service::attempt_reads;
use crate::service::attempts::{self, ListQuery};
use crate::service::{run_flow, runs, scheduler};
use crate::spec::routes::{self, SPECS, strings};

/// The routes of the attempt, run and scheduler operations, in registration order.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: run_exec,
};

/// The reason a cancel gives when its body does not name one (`CancelBody.reason`).
const CANCEL_REASON: &str = "cancelled from the web UI";

/// One operation on the application context: the request in, the document out.
fn run_exec(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params,
        body,
        context,
        ..
    } = request;
    let ws = text(&params, "ws").unwrap_or_default();
    let attempt = text(&params, "attempt_id").unwrap_or_default();
    match id {
        "H071" => {
            let statuses = strings(&params, "status");
            let query = ListQuery {
                workspace: Some(ws),
                task: text(&params, "task"),
                statuses: &statuses,
                provider: text(&params, "provider"),
                limit: number(&params, "limit", 50),
                offset: number(&params, "offset", 0),
            };
            attempts::attempt_list(app, &context, &query)
        }
        "H072" => {
            let request = routes::start_request(&body);
            let task = body_text(&body, "task").unwrap_or_default();
            run_flow::run_start(app, &context, &mut LiveAgents, Some(ws), task, &request)
        }
        "H073" => attempts::attempt_compare(
            app,
            &context,
            text(&params, "a").unwrap_or_default(),
            text(&params, "b").unwrap_or_default(),
        ),
        "H074" => {
            app.require_permission(&context, Permission::TaskRead)?;
            runs::get(app, attempt, number(&params, "events", 100))
        }
        "H075" => attempts::attempt_manifests(app, &context, attempt),
        "H076" => attempt_reads::log(app, &context, attempt, number(&params, "tail", 400)),
        "H077" => {
            let reason = body_text(&body, "reason").unwrap_or(CANCEL_REASON);
            run_flow::run_cancel(app, &context, attempt, reason)
        }
        "H078" => run_flow::run_retry(app, &context, &mut LiveAgents, attempt, None),
        "H079" => run_flow::run_rerun(app, &context, &mut LiveAgents, attempt),
        "H122" => {
            app.require_permission(&context, Permission::TaskRead)?;
            let workspace = app.workspace_id(ws)?;
            scheduler::overview(app, Some(workspace))
        }
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

/// A string parameter of a query; `None` when unset.
fn text<'a>(params: &'a Map<String, Json>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Json::as_str)
}

/// A string member of a body; `None` when the body is absent or the member is not a string.
fn body_text<'a>(body: &'a Json, name: &str) -> Option<&'a str> {
    body.get(name).and_then(Json::as_str)
}

/// An integer parameter, `default` when unset.
fn number(params: &Map<String, Json>, name: &str, default: i64) -> i64 {
    params.get(name).and_then(Json::as_i64).unwrap_or(default)
}
