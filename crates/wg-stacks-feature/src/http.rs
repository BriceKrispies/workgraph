//! The stacks group (`api/routes/stacks.py`): `GET /stacks` (H062), `POST /stacks` (H063), `GET
//! /stacks/{ref}` (H064), `POST /stacks/{ref}/items` (H065), `DELETE /stacks/{ref}/items/{task}`
//! (H066), `GET /stacks/{ref}/history` (H067), `GET /tasks/{ref}/stacks` (H068), `POST
//! /stacks/{ref}/integrate` (H069) and `POST /stacks/{ref}/verify` (H070), all under
//! `/api/v1/ws/{ws}`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry point.

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::{ErrorCode, WgError};

use crate::service;
use crate::spec::body_text;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 9] = [
    RouteSpec {
        id: "H062",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks",
        status: 200,
        query: &["include_inactive"],
        body: None,
    },
    RouteSpec {
        id: "H063",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "StackCreate",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H064",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H065",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/items",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "StackItemAdd",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H066",
        method: "DELETE",
        path: "/api/v1/ws/{ws}/stacks/{ref}/items/{task}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H067",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks/{ref}/history",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H068",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/stacks",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H069",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/integrate",
        status: 202,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H070",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/verify",
        status: 202,
        query: &[],
        body: None,
    },
];

/// The group: its table and the function that runs an operation.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
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
            service::list(app, context, ws, all)
        }
        "H063" => service::create(app, context, ws, body_text(&request.body, "parent")),
        "H064" => service::show(app, context, ws, reference),
        "H065" => service::add(
            app,
            context,
            ws,
            reference,
            body_text(&request.body, "task"),
        ),
        "H066" => service::remove(app, context, ws, reference, text_param(params, "task")),
        "H067" => service::history(app, context, ws, reference),
        "H068" => service::containing(app, context, ws, reference),
        "H069" => service::integrate(app, context, ws, reference),
        "H070" => service::verify(app, context, ws, reference),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stacks_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H062", "H063", "H064", "H065", "H066", "H067", "H068", "H069", "H070"
            ]
        );
        for spec in &SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-53");
        }
    }

    #[test]
    fn the_stack_bodies_load_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &SPECS {
            if let Some(body) = &spec.body {
                assert!(
                    wg_serverspec::groups::body_spec(&openapi, body).is_some(),
                    "{}",
                    spec.id
                );
            }
        }
    }
}
