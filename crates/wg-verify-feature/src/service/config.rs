//! Shell: `verify config`, the creation or update of a verification configuration
//! (`VerificationService.create_config`).

use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_store::Session;
use wg_types::{Id, Permission, WgError};

use crate::spec::config_write::{ConfigRequest, ConfigWorld, write_config};

fn load(session: &mut Session<'_>, workspace: Id) -> Result<ConfigWorld, WgError> {
    Ok(ConfigWorld {
        tasks: session.task_world(Some(workspace), &[])?.tasks,
        configs: session.verification_configs(workspace)?,
    })
}

/// Creates the configuration of the scope and name, or updates it. The reply is the
/// `VerificationConfigOut` document.
pub fn config(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &ConfigRequest<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ConfigManage)?;
    let ws = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide(
            context,
            |session| load(session, ws),
            |world, stamp| write_config(world, stamp, ws, request),
        )?
        .reply)
}
