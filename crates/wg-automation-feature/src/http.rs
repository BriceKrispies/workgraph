//! The automation group (`api/routes/ops.py`, automation and notifications): the catalog (H091),
//! the rules (H092 to H096), the dry run (H097), the run history (H098), the notifications (H099)
//! and their read mark (H100), under `/api/v1`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry points.

use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::{BodyRef, OPEN_OBJECT, RouteSpec};
use wg_types::{ErrorCode, WgError};
use wg_wire::convention::dict;

use crate::service;
use crate::spec::catalog;
use crate::spec::routes::{action_inputs, preview_args, run_args, unread_only};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 10] = [
    RouteSpec {
        id: "H091",
        method: "GET",
        path: "/api/v1/automation/catalog",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H092",
        method: "GET",
        path: "/api/v1/ws/{ws}/automation/rules",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H093",
        method: "POST",
        path: "/api/v1/ws/{ws}/automation/rules",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: OPEN_OBJECT,
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H094",
        method: "GET",
        path: "/api/v1/ws/{ws}/automation/rules/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H095",
        method: "PUT",
        path: "/api/v1/ws/{ws}/automation/rules/{ref}",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: OPEN_OBJECT,
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H096",
        method: "POST",
        path: "/api/v1/ws/{ws}/automation/rules/{ref}/{action}",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: OPEN_OBJECT,
            strip: false,
            required: false,
        }),
    },
    RouteSpec {
        id: "H097",
        method: "POST",
        path: "/api/v1/ws/{ws}/automation/preview",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "PreviewBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H098",
        method: "GET",
        path: "/api/v1/ws/{ws}/automation/runs",
        status: 200,
        query: &["rule", "status", "limit"],
        body: None,
    },
    RouteSpec {
        id: "H099",
        method: "GET",
        path: "/api/v1/ws/{ws}/notifications",
        status: 200,
        query: &["unread"],
        body: None,
    },
    RouteSpec {
        id: "H100",
        method: "POST",
        path: "/api/v1/notifications/{notification_id}/read",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let context = &request.context;
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let reference = text_param(params, "ref");
    match id {
        "H091" => Ok(catalog()),
        "H092" => service::list(app, context, ws),
        "H093" => service::create(app, context, ws, &request.body),
        "H094" => service::get(app, context, ws, reference),
        "H095" => service::update(app, context, ws, reference, &request.body),
        "H096" => rule_action(
            app,
            context,
            ws,
            reference,
            text_param(params, "action"),
            &request.body,
        ),
        "H097" => {
            let args = preview_args(&request.body);
            service::preview(
                app,
                context,
                ws,
                args.rule.as_deref(),
                args.spec.as_ref(),
                args.event_id.as_deref(),
                &args.inputs,
            )
        }
        "H098" => {
            let (rule, status, limit) = run_args(params);
            service::runs(app, context, ws, rule, status, limit)
        }
        "H099" => service::notifications(app, context, ws, unread_only(params)),
        "H100" => {
            service::mark_read(app, context, text_param(params, "notification_id"))?;
            Ok(dict(vec![("ok", Value::Bool(true))]))
        }
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

/// `POST /ws/{ws}/automation/rules/{ref}/{action}`: enable or disable the rule (its document), or
/// trigger it with the body's inputs (its run document).
fn rule_action(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    reference: &str,
    action: &str,
    body: &serde_json::Value,
) -> Result<Value, WgError> {
    match action {
        "enable" => service::set_enabled(app, context, ws, reference, true),
        "disable" => service::set_enabled(app, context, ws, reference, false),
        "trigger" => service::trigger(app, context, ws, reference, &action_inputs(body)),
        other => Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("unknown rule action {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_automation_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H091", "H092", "H093", "H094", "H095", "H096", "H097", "H098", "H099", "H100"
            ]
        );
        for spec in &SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-47");
        }
    }

    #[test]
    fn the_automation_bodies_load_from_the_snapshot() {
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
