//! The API route groups (plan section 9.9).
//!
//! A group task adds a module here with `pub fn register(table)` for its operations and chains it
//! into [`routes_with`] (which `ServerOptions::routes` takes). A feature contributes its routes as
//! `wg_runtime::RouteGroup` tables through the `&[&Feature]` the binary passes. The route-table
//! test derives the implemented groups from the registered routes and requires every operation of
//! those groups to be registered as the OpenAPI snapshot declares it; groups not implemented are
//! reported, not failed (plan 5.5).
//!
//! A handler parses nothing itself: the path and query parameters arrive validated against the
//! snapshot, and the body against the route's [`crate::BodySpec`]. It calls the runtime through
//! [`crate::Server::blocking`] and returns an [`crate::Answer`].

pub mod events;
pub mod meta_settings;

use wg_runtime::{Feature, RouteRequest, RouteRun};
use wg_serverspec::groups::{RouteSpec, body_spec};

use crate::route::{Answer, ApiRoute, handler};

/// Registers a table of operations into `table`, in the table's order: each operation's handler
/// runs `run` with the operation's id on the application context and answers its document with
/// the operation's success status.
pub fn bind(table: &mut Vec<ApiRoute>, specs: &[RouteSpec], run: RouteRun) {
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
                // The validated parts of the API request, as the group's `run` receives them.
                let route = RouteRequest {
                    params: request.params,
                    body: request.body,
                    given: request.given,
                    context: request.context,
                };
                let value = server.blocking(move |app| run(app, id, route)).await?;
                Ok(Answer::Json(value))
            }),
        });
    }
}

/// Every registered API route: the groups that predate the feature descriptor, in their order,
/// then the routes of `features` in order.
pub fn routes_with(features: &[&Feature]) -> Vec<ApiRoute> {
    let mut table = Vec::new();
    meta_settings::register(&mut table);
    events::register(&mut table);
    for feature in features {
        for group in feature.routes {
            bind(&mut table, group.specs, group.run);
        }
    }
    table
}
