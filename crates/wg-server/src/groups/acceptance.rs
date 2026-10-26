//! The acceptance group (plan IF-24): `POST /tasks/{ref}/accept` (H033), `POST
//! /tasks/{ref}/reject` (H034) and `GET /tasks/{ref}/acceptances` (H035), under `/api/v1/ws/{ws}`.
//!
//! The table of operations is `wg-serverspec`'s ([`ACCEPTANCE`]); the rules are the application's
//! (`App::task_accept`, `task_accept_manually`, `task_reject`, `task_acceptances`).

use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::tasks::acceptance::body_parts;
use wg_runtimespec::tasks::query::addressed;
use wg_serverspec::groups::ACCEPTANCE;
use wg_types::{ErrorCode, WgError};

use super::bind;
use crate::route::ApiRoute;
use wg_runtime::RouteRequest;

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    bind(table, &ACCEPTANCE, run);
}

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let (ws, reference) = addressed(&request.params);
    let ws = Some(ws);
    let context = &request.context;
    let (manual, reason) = body_parts(&request.body);
    match id {
        "H033" if manual => app.task_accept_manually(context, ws, reference, &reason),
        "H033" => app.task_accept(context, ws, reference, &reason),
        "H034" => app.task_reject(context, ws, reference, &reason),
        "H035" => app.task_acceptances(context, ws, reference),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}
