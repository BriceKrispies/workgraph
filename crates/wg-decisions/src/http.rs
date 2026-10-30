//! The decisions group (`api/routes/decisions.py`): `GET /decisions` (H125), `GET
//! /decisions/{decision_id}` (H126), `POST /decisions/{decision_id}/choose` (H127), `POST
//! /decisions/{decision_id}/withdraw` (H128), `GET /tasks/{ref}/decisions` (H129) and `POST
//! /tasks/{ref}/decisions` (H130), all under `/api/v1/ws/{ws}`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry point.

use wg_iface::http::groups::{BodyRef, RouteSpec};
use wg_json::Value;
use wg_runtime::spec::text_param;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_types::WgError;

use crate::service;
use crate::spec::{choose_parts, items, list_args, request_parts, withdraw_reason};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 6] = [
    RouteSpec {
        id: "H125",
        method: "GET",
        path: "/api/v1/ws/{ws}/decisions",
        status: 200,
        query: &["status", "limit"],
        body: None,
    },
    RouteSpec {
        id: "H126",
        method: "GET",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H129",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/decisions",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H130",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/decisions",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionRequest",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H127",
        method: "POST",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}/choose",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionChoose",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H128",
        method: "POST",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}/withdraw",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionWithdraw",
            strip: true,
            required: false,
        }),
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: |app, id, request| run(app, id, request).map(RouteAnswer::Json),
};

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let context = &request.context;
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let decision_id = text_param(params, "decision_id");
    match id {
        "H125" => {
            let (filter, limit) = list_args(params);
            service::list(app, context, ws, filter, limit).map(items)
        }
        "H126" => service::get(app, context, ws, decision_id),
        "H129" => service::for_task(app, context, ws, text_param(params, "ref")).map(items),
        "H130" => {
            let (question, options) = request_parts(&request.body);
            service::request(
                app,
                context,
                ws,
                text_param(params, "ref"),
                &question,
                &options,
            )
        }
        "H127" => {
            let [option, other, note] = choose_parts(&request.body);
            service::choose(
                app,
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
            service::withdraw(app, context, ws, decision_id, reason.as_deref())
        }
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decisions_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H125", "H126", "H129", "H130", "H127", "H128"]);
        for spec in &SPECS {
            assert_eq!(wg_iface::http::operations::group_of(spec.id), "decisions");
        }
    }

    #[test]
    fn the_decision_bodies_load_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &SPECS {
            if let Some(body) = &spec.body {
                assert!(
                    wg_iface::http::groups::body_spec(&openapi, body).is_some(),
                    "{}",
                    spec.id
                );
            }
        }
    }
}
