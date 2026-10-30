//! The verification group (`api/routes/ops.py`, verification): the check configs (H085, H086),
//! their enable and disable (H087), the check runs (H088, H089) and the failure history (H090),
//! under `/api/v1`, and the task's verification runs, applicable checks and verify job (H057 to
//! H059, from `api/routes/tasks.py`).
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service::api`] entry points.

use serde_json::Value as Json;
use wg_iface::http::groups::{BodyRef, RouteSpec};
use wg_json::Value;
use wg_runtime::spec::text_param;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_types::WgError;

use crate::service::api;
use crate::spec::config_write::config_body;
use crate::spec::routes::{enabled_of, failure_args, run_args};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 9] = [
    RouteSpec {
        id: "H085",
        method: "GET",
        path: "/api/v1/ws/{ws}/verification/configs",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H086",
        method: "POST",
        path: "/api/v1/ws/{ws}/verification/configs",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "VerificationConfigBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H087",
        method: "POST",
        path: "/api/v1/verification/configs/{config_id}/{action}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H088",
        method: "GET",
        path: "/api/v1/ws/{ws}/verification/runs",
        status: 200,
        query: &["status", "limit"],
        body: None,
    },
    RouteSpec {
        id: "H089",
        method: "GET",
        path: "/api/v1/verification/runs/{run_id}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H090",
        method: "GET",
        path: "/api/v1/ws/{ws}/verification/failures",
        status: 200,
        query: &["check", "repository", "since"],
        body: None,
    },
    RouteSpec {
        id: "H057",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/verification",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H058",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/checks",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H059",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/verify",
        status: 200,
        query: &["attempt_id", "tier"],
        body: None,
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
    let ws = text_param(params, "ws");
    match id {
        "H085" => api::configs(app, context, ws),
        "H086" => api::create_config(app, context, ws, &config_body(&request.body)),
        "H087" => {
            let enabled = enabled_of(text_param(params, "action"))?;
            api::toggle(app, context, text_param(params, "config_id"), enabled)
        }
        "H088" => {
            let (status, limit) = run_args(params);
            api::runs(app, context, ws, status, limit)
        }
        "H089" => api::run(app, context, text_param(params, "run_id")),
        "H090" => {
            let (check, repository, since) = failure_args(params);
            api::failures(app, context, ws, check, repository, since)
        }
        "H057" => api::task_runs(app, context, ws, text_param(params, "ref")),
        "H058" => api::checks_for_task(app, context, ws, text_param(params, "ref")),
        "H059" => api::enqueue_verify(
            app,
            context,
            ws,
            text_param(params, "ref"),
            params.get("attempt_id").and_then(Json::as_str),
            params.get("tier").and_then(Json::as_str),
        ),
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers: H057 H058 H059
    #[test]
    fn the_verification_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H085", "H086", "H087", "H088", "H089", "H090", "H057", "H058", "H059"
            ]
        );
        for spec in &SPECS {
            let group = if matches!(spec.id, "H057" | "H058" | "H059") {
                "IF-45"
            } else {
                "IF-47"
            };
            assert_eq!(wg_iface::http::operations::group_of(spec.id), group);
        }
    }

    #[test]
    fn the_verification_bodies_load_from_the_snapshot() {
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
