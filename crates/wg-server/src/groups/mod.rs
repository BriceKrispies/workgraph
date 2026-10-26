//! The API route groups (plan section 9.9).
//!
//! A group task adds a module here with `pub fn routes() -> Vec<ApiRoute>` for its operations,
//! chains it into [`routes`] (which `ServerOptions::routes` takes), and adds its group name
//! (`wg_serverspec::operations::group_of`) to [`IMPLEMENTED`]. The route-table test then requires
//! every operation of the group to be registered as the OpenAPI snapshot declares it; groups not
//! listed are reported, not failed (plan 5.5).
//!
//! A handler parses nothing itself: the path and query parameters arrive validated against the
//! snapshot, and the body against the route's [`crate::BodySpec`]. It calls the runtime through
//! [`crate::Server::blocking`] and returns an [`crate::Answer`].

pub mod decisions;
pub mod meta_settings;

use wg_json::Value;
use wg_runtime::App;
use wg_serverspec::groups::{RouteSpec, body_spec};
use wg_types::WgError;

use crate::route::{Answer, ApiRequest, ApiRoute, handler};

/// The groups whose routes are registered.
pub const IMPLEMENTED: &[&str] = &["meta-settings", "decisions"];

/// What a group runs for one operation: the application context, the operation's inventory id
/// and the validated request, in; the document, out.
pub type Run = fn(&mut App, &str, ApiRequest) -> Result<Value, WgError>;

/// Registers a table of operations into `table`, in the table's order: each operation's handler
/// runs `run` with the operation's id on the application context and answers its document with
/// the operation's success status.
pub fn bind(table: &mut Vec<ApiRoute>, specs: &[RouteSpec], run: Run) {
    let openapi = wg_wire::contract::openapi().ok();
    for spec in specs {
        let body = spec
            .body
            .as_ref()
            .and_then(|body| openapi.as_ref().and_then(|doc| body_spec(doc, body)));
        let id = spec.id;
        table.push(ApiRoute {
            id,
            method: spec.method,
            path: spec.path,
            status: spec.status,
            query: spec.query,
            body,
            handler: handler(move |server, request| async move {
                let value = server.blocking(move |app| run(app, id, request)).await?;
                Ok(Answer::Json(value))
            }),
        });
    }
}

/// Every registered API route, from the implemented groups.
pub fn routes() -> Vec<ApiRoute> {
    let mut table = Vec::new();
    meta_settings::register(&mut table);
    decisions::register(&mut table);
    table
}
