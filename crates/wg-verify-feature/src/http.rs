//! The verification group (`api/routes/ops.py`, verification): the check configs (H085, H086),
//! their enable and disable (H087), the check runs (H088, H089) and the failure history (H090),
//! under `/api/v1`.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service::api`] entry points.

use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::{ErrorCode, WgError};

use crate::service::api;
use crate::spec::config_write::config_body;
use crate::spec::routes::{enabled_of, failure_args, run_args};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 6] = [
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
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

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
    fn the_verification_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H085", "H086", "H087", "H088", "H089", "H090"]);
        for spec in &SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-47");
        }
    }

    #[test]
    fn the_verification_bodies_load_from_the_snapshot() {
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
