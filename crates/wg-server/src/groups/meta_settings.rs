//! The meta and settings group (plan IF-42, first part): `GET /api/v1/health` (H001), `GET
//! /api/v1/meta` (H002), `GET` and `PATCH /api/v1/settings` (H117, H118).
//!
//! The table of operations is `wg-serverspec`'s ([`META_SETTINGS`]); the rules are
//! `wg-runtimespec`'s and the documents are `wg-wire`'s. This module is the glue: it binds each
//! operation to its handler and runs the handler on the application context, which is a `&mut`
//! operation (the shell's job).

use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::settings::{meta_inputs, patch_values, patched_config, settings_workspace};
use wg_serverspec::groups::{META_SETTINGS, body_spec};
use wg_types::{ErrorCode, Permission, WgError};
use wg_wire::convention::{json_object, json_value};
use wg_wire::dto::{health, settings as settings_dto};

use crate::route::{Answer, ApiRequest, ApiRoute, handler};

/// The version the documents report (`workgraph.__version__`).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    let openapi = wg_wire::contract::openapi().ok();
    for spec in &META_SETTINGS {
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

/// One operation on the application context: the request in, the document out. The
/// permission checks come first, as the Python routes make them.
fn run(app: &mut App, id: &str, request: ApiRequest) -> Result<Value, WgError> {
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
