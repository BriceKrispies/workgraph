//! The Claude Code route (H121, `api/routes/ops.py` `claude_status`): `GET /api/v1/claude/status`,
//! the user-scope hook installation, the ingestion statistics and the twenty newest sessions.
//!
//! [`SPECS`] is the table of the operation; the server binds it to [`run`], which checks the
//! permission and calls [`document`]. The document is the status DTO of `wg-wire`.

use wg_json::Value;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_serverspec::groups::RouteSpec;
use wg_types::{Permission, WgError};
use wg_wire::convention::json_value;
use wg_wire::dto::claude;

use crate::service::claude::{ClaudeEnv, installation};
use crate::service::hooks;

/// The operations of the group: H121.
pub const SPECS: [RouteSpec; 1] = [RouteSpec {
    id: "H121",
    method: "GET",
    path: "/api/v1/claude/status",
    status: 200,
    query: &[],
    body: None,
}];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: |app, id, request| run(app, id, request).map(RouteAnswer::Json),
};

/// One operation on the application context: the permission check first, then the document.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H121" => {
            request.context.require(&[Permission::WorkspaceRead])?;
            document(app, &ClaudeEnv::from_process())
        }
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

/// The status document: `user` (the user-scope installation), `hooks` (the ingestion statistics)
/// and `sessions` (the twenty newest sessions), read from `env` and `app`.
pub fn document(app: &mut App, env: &ClaudeEnv) -> Result<Value, WgError> {
    let status = installation(env, "user", None);
    let user = claude::installer_status(
        &status.scope,
        &status.settings,
        status.exists,
        status.error.as_deref(),
        &status.installed_events,
        &status.missing_events,
        status.command_resolves,
    );
    let hooks = json_value(&hooks::hook_stats(app)?);
    let sessions = hooks::hook_sessions(app, None, 20)?
        .iter()
        .map(json_value)
        .collect();
    Ok(claude::status(user, hooks, sessions))
}
