//! The decisions group (`api/routes/decisions.py`): `GET /decisions` (H125), `GET
//! /decisions/{decision_id}` (H126), `POST /decisions/{decision_id}/choose` (H127), `POST
//! /decisions/{decision_id}/withdraw` (H128), `GET /tasks/{ref}/decisions` (H129) and `POST
//! /tasks/{ref}/decisions` (H130), all under `/api/v1/ws/{ws}`.
//!
//! The table of operations is `wg-serverspec`'s ([`DECISIONS`]); the readers of the requests and
//! the output values are `wg-runtimespec`'s. This module binds each operation to its `App` entry
//! point, which is a `&mut` operation (the shell's job).

use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::decisions::{
    choose_parts, items, list_args, request_parts, text_param, withdraw_reason,
};
use wg_serverspec::groups::DECISIONS;
use wg_types::{ErrorCode, WgError};

use super::bind;
use crate::route::ApiRoute;
use wg_runtime::RouteRequest;

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    bind(table, &DECISIONS, run);
}

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let context = &request.context;
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let decision_id = text_param(params, "decision_id");
    match id {
        "H125" => {
            let (filter, limit) = list_args(params);
            app.decision_list(context, ws, filter, limit).map(items)
        }
        "H126" => app.decision_get(context, ws, decision_id),
        "H129" => app
            .decision_for_task(context, ws, text_param(params, "ref"))
            .map(items),
        "H130" => {
            let (question, options) = request_parts(&request.body);
            app.decision_request(context, ws, text_param(params, "ref"), &question, &options)
        }
        "H127" => {
            let [option, other, note] = choose_parts(&request.body);
            app.decision_choose(
                context,
                ws,
                decision_id,
                option.as_deref(),
                other.as_deref(),
                note.as_deref(),
            )
        }
        "H128" => {
            let reason = withdraw_reason(&request.body);
            app.decision_withdraw(context, ws, decision_id, reason.as_deref())
        }
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}
