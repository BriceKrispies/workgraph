//! The workspace and settings routes of the registry feature: `GET /api/v1/workspaces` (H003),
//! `POST /api/v1/workspaces` (H004) and `GET /api/v1/workspaces/{ws}` (H005) from
//! `api/routes/core.py`, and `GET /api/v1/settings` (H117) and `PATCH /api/v1/settings` (H118)
//! from `api/routes/ops.py`. The health and meta routes stay in the server's base group.
//!
//! [`SPECS`] is the table of operations; the server binds each one to [`run`], which reads the
//! validated request and calls the feature's [`crate::service`] entry point or the rules of
//! [`crate::spec::settings`] on the application. The workspace routes take no permission check
//! of their own: the service makes it, as the Python service does.

use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::{ErrorCode, Permission, WgError};
use wg_wire::convention::json_object;

use crate::service;
use crate::spec::dto::settings as settings_dto;
use crate::spec::settings::{patch_values, patched_config, settings_workspace};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 5] = [
    RouteSpec {
        id: "H003",
        method: "GET",
        path: "/api/v1/workspaces",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H004",
        method: "POST",
        path: "/api/v1/workspaces",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "WorkspaceCreate",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H005",
        method: "GET",
        path: "/api/v1/workspaces/{ws}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H117",
        method: "GET",
        path: "/api/v1/settings",
        status: 200,
        query: &["workspace"],
        body: None,
    },
    RouteSpec {
        id: "H118",
        method: "PATCH",
        path: "/api/v1/settings",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "SettingsPatch",
            required: true,
            strip: false,
        }),
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out. The permission
/// checks come first, as the Python routes make them.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H003" => service::workspace_list(app, &request.context, false),
        "H004" => service::workspace_create(app, &request.context, &request.body, false),
        "H005" => service::workspace_summary(
            app,
            &request.context,
            Some(text_param(&request.params, "ws")),
        ),
        "H117" => {
            request.context.require(&[Permission::WorkspaceRead])?;
            let config = app.config()?;
            let slug = settings_workspace(&request.params, &config);
            let effective = app.effective_config(slug.as_deref())?;
            Ok(settings_dto::settings(
                json_object(&effective),
                app.paths.config_file().as_str(),
                app.paths.data_dir().as_str(),
            ))
        }
        "H118" => {
            request.context.require(&[Permission::ConfigManage])?;
            let current = app.config()?;
            let next = patched_config(&current, &patch_values(&request.body))?;
            app.save_config(&next)?;
            Ok(settings_dto::patched(json_object(&next)))
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
    fn the_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H003", "H004", "H005", "H117", "H118"]);
        for spec in &SPECS {
            let expected = match spec.id {
                "H117" | "H118" => "meta-settings",
                _ => "IF-42",
            };
            assert_eq!(
                wg_serverspec::operations::group_of(spec.id),
                expected,
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn the_workspace_create_body_strips_whitespace() {
        let create = SPECS.iter().find(|spec| spec.id == "H004");
        let body = create.and_then(|spec| spec.body.as_ref());
        assert!(body.is_some_and(|body| body.strip && body.required));
    }

    #[test]
    fn the_settings_patch_body_loads_from_the_snapshot() {
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
