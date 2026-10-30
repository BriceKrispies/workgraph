//! The meta group (plan IF-42, first part): `GET /api/v1/health` (H001) and `GET /api/v1/meta`
//! (H002). The settings routes (H117, H118) are the registry feature's.
//!
//! The table of operations is `wg-serverspec`'s ([`META_SETTINGS`]); the rules are
//! `wg-runtime`'s `spec`'s and the documents are `wg-wire`'s. This module is the glue: it binds each
//! operation to its handler and runs the handler on the application context, which is a `&mut`
//! operation (the shell's job).

use wg_iface::http::groups::META_SETTINGS;
use wg_json::Value;
use wg_runtime::App;
use wg_runtime::spec::settings::meta_inputs;
use wg_types::WgError;
use wg_wire::convention::json_value;
use wg_wire::dto::health;

use super::bind;
use crate::route::ApiRoute;
use wg_runtime::{RouteAnswer, RouteRequest};

/// The version the documents report (`workgraph.__version__`).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    bind(table, &META_SETTINGS, |app, id, request| {
        run(app, id, request).map(RouteAnswer::Json)
    });
}

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, _request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H001" => Ok(health::health(VERSION)),
        "H002" => {
            let inputs = meta_inputs(&app.config()?);
            Ok(health::meta(
                VERSION,
                inputs.default_workspace.as_deref(),
                inputs.execution_enabled,
                inputs.auto_schedule,
                &inputs.agents,
                json_value(&inputs.classification),
                json_value(&inputs.embeddings),
            ))
        }
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}
