//! The task reads, writes and lifecycle group (plan IF-44): H023 to H046 without the acceptance
//! routes H033 to H035 (IF-24).
//!
//! The table of operations is `wg-serverspec`'s ([`TASKS`]); the rules are the application's
//! (`App::task_*`, over `wg-runtimespec`'s decisions) and the documents are `wg-wire`'s. This
//! module is the glue: it binds each operation to its handler and runs the handler on the
//! application context, which is a `&mut` operation (the shell's job).

use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::tasks::query::{
    addressed, filter_body, first_shown, flag, graph_request, number, text,
};
use wg_serverspec::groups::TASKS;
use wg_types::{ErrorCode, WgError};

use wg_runtime::RouteRequest;

use super::bind;
use crate::route::ApiRoute;

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    bind(table, &TASKS, run);
}

/// One operation on the application context: the request in, the document out. The permission
/// checks are the application's, made before anything is looked up as the Python services do.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params,
        given,
        context,
        ..
    } = request;
    let (ws, reference) = addressed(&params);
    let ws = Some(ws);
    match id {
        "H023" => app.task_list(&context, ws, &filter_body(&params)),
        "H024" => app.task_create(&context, ws, &given),
        "H025" => app.task_bulk_update(&context, ws, &given),
        "H026" => app.task_ready(
            &context,
            ws,
            text(&params, "project"),
            text(&params, "repository"),
            None,
            number(&params, "limit", 50),
        ),
        "H027" => app.task_graph(&context, ws, &graph_request(&params)),
        "H028" => first_shown(app.task_show(&context, ws, &[reference.to_owned()])?),
        "H029" => app.task_update(&context, ws, reference, &given),
        "H030" => app.task_claim(&context, ws, reference, &given),
        "H031" => app.task_release(&context, ws, reference, &given),
        "H032" => app.task_complete(&context, ws, reference, &given),
        "H036" => app.task_comment(&context, ws, reference, &given),
        "H037" => app.task_progress(&context, ws, reference, &given),
        "H038" => app.task_result(&context, ws, reference, &given),
        "H039" => app.task_add_dependency(&context, ws, reference, &given),
        "H040" => app.task_remove_dependency(
            &context,
            ws,
            reference,
            text(&params, "depends_on").unwrap_or_default(),
        ),
        "H041" => app.task_add_relation(&context, ws, reference, &given),
        "H042" => app.task_link_repository(&context, ws, reference, &given),
        "H043" => app.task_unlink_repository(
            &context,
            ws,
            reference,
            text(&params, "repository").unwrap_or_default(),
        ),
        "H044" => app.task_archive(&context, ws, reference, !flag(&params, "undo", false)),
        "H045" => {
            app.task_history_limited(&context, ws, reference, &[], number(&params, "limit", 500))
        }
        "H046" => app.task_attempts(
            &context,
            ws,
            reference,
            number(&params, "limit", 50),
            number(&params, "offset", 0),
        ),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}
