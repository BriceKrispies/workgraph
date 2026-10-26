//! The stacks group (`api/routes/stacks.py`): `GET /stacks` (H062), `POST /stacks` (H063), `GET
//! /stacks/{ref}` (H064), `POST /stacks/{ref}/items` (H065), `DELETE /stacks/{ref}/items/{task}`
//! (H066), `GET /stacks/{ref}/history` (H067), `GET /tasks/{ref}/stacks` (H068), `POST
//! /stacks/{ref}/integrate` (H069) and `POST /stacks/{ref}/verify` (H070), all under
//! `/api/v1/ws/{ws}`.
//!
//! The table of operations is `wg-serverspec`'s ([`STACKS`]); the stack outputs and decisions are
//! `wg-runtimespec`'s. This module binds each operation to its `App` entry point, which is a
//! `&mut` operation (the shell's job).

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::decisions::text_param;
use wg_runtimespec::stacks::body_text;
use wg_serverspec::groups::STACKS;
use wg_types::{ErrorCode, WgError};

use super::bind;
use crate::route::{ApiRequest, ApiRoute};

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    bind(table, &STACKS, run);
}

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: ApiRequest) -> Result<Value, WgError> {
    let context = &request.context;
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let reference = text_param(params, "ref");
    match id {
        "H062" => {
            let all = params
                .get("include_inactive")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            app.stack_list(context, ws, all)
        }
        "H063" => app.stack_create(context, ws, body_text(&request.body, "parent")),
        "H064" => app.stack_show(context, ws, reference),
        "H065" => app.stack_add(context, ws, reference, body_text(&request.body, "task")),
        "H066" => app.stack_remove(context, ws, reference, text_param(params, "task")),
        "H067" => app.stack_history(context, ws, reference),
        "H068" => app.stack_containing(context, ws, reference),
        "H069" => app.stack_integrate(context, ws, reference),
        "H070" => app.stack_verify(context, ws, reference),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}
