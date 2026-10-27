//! Shell: the verification entry points of the `/verification` routes (H085 to H090,
//! `VerificationService` as the API calls it). Each checks its permission, reads or writes in the
//! transaction of the call and answers the wire documents the Python service returns. The
//! decisions are [`crate::spec::config_write`] and the reads are [`crate::service::reads`].

use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::{VerificationConfig, VerificationRun};
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, VerificationStatus, WgError, parse_id};
use wg_wire::dto::verification::{config_from_model, failure_row};

use crate::service::config;
use crate::service::listing::run_document;
use crate::service::reads;
use crate::spec::config_write::{self, ConfigBody, ConfigRequest};
use crate::spec::routes::parse_since;
use crate::spec::sql::{FailureFilter, Page, RunFilter};

/// The most failures `GET /ws/{ws}/verification/failures` returns (`failures`' default limit).
const FAILURE_LIMIT: i64 = 200;

/// A permission check against a fresh stamp, before anything is looked up.
fn require(app: &mut App, context: &RequestContext, permission: Permission) -> Result<(), WgError> {
    app.runtime.stamps.stamp(context)?.require(&[permission])
}

/// `VerificationService.list_configs`: the configs of the workspace, by scope type and name.
pub fn configs(app: &mut App, context: &RequestContext, workspace: &str) -> Result<Value, WgError> {
    require(app, context, Permission::WorkspaceRead)?;
    let ws = app.workspace_or_default(Some(workspace))?;
    let configs = app
        .runtime
        .store
        .read(|session| session.verification_configs(ws))?;
    Ok(Value::List(configs.iter().map(config_from_model).collect()))
}

/// `VerificationService.create_config`: writes the configuration and replies with it.
pub fn create_config(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
    body: &ConfigBody,
) -> Result<Value, WgError> {
    // Python's order: the permission, then the checks, then the workspace is resolved.
    require(app, context, Permission::ConfigManage)?;
    config_write::check_specs(&body.checks)?;
    let request = ConfigRequest {
        scope_type: &body.scope_type,
        scope: body.scope.as_deref(),
        name: &body.name,
        checks: &body.checks,
    };
    config::config(app, context, Some(workspace), &request)
}

/// `VerificationService.set_config_enabled`: switches a configuration on or off. An id that names
/// no configuration is `NotFound`.
pub fn toggle(
    app: &mut App,
    context: &RequestContext,
    config_id: &str,
    enabled: bool,
) -> Result<Value, WgError> {
    require(app, context, Permission::ConfigManage)?;
    let id = parse_id(config_id).ok();
    Ok(app
        .runtime
        .decide(
            context,
            |session| match id {
                Some(found) => {
                    session.fetch::<VerificationConfig>(&RowKey::VerificationConfig(found))
                }
                None => Ok(None),
            },
            |found, stamp| config_write::toggle(found.clone(), config_id, stamp, enabled),
        )?
        .reply)
}

/// `VerificationService.list_runs` over the API's filters: the newest runs of the workspace, by
/// status (any status when none is named; an unknown status matches no run).
pub fn runs(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
    status: Option<&str>,
    limit: i64,
) -> Result<Value, WgError> {
    require(app, context, Permission::TaskRead)?;
    let ws = app.workspace_or_default(Some(workspace))?;
    let status = match status {
        Some(text) => match VerificationStatus::parse(text) {
            Some(known) => Some(known),
            None => return Ok(Value::List(Vec::new())),
        },
        None => None,
    };
    let filter = RunFilter {
        task_id: None,
        attempt_id: None,
        status,
    };
    let page = Page { limit, offset: 0 };
    app.runtime.store.read(|session| {
        let runs = reads::verification_runs(session, ws, &filter, page)?;
        let documents = runs
            .iter()
            .map(|run| run_document(session, run))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Value::List(documents))
    })
}

/// `VerificationService.get_run`: one run with its manifest entries and check results.
pub fn run(app: &mut App, context: &RequestContext, run_id: &str) -> Result<Value, WgError> {
    require(app, context, Permission::TaskRead)?;
    let id = parse_id(run_id).ok();
    app.runtime.store.read(|session| {
        let found = match id {
            Some(found) => session.fetch::<VerificationRun>(&RowKey::VerificationRun(found))?,
            None => None,
        };
        let run = found
            .ok_or_else(|| WgError::not_found(format!("verification run {run_id} not found")))?;
        run_document(session, &run)
    })
}

/// `VerificationService.failures`: the failed and errored check results of the workspace, newest
/// first, optionally of one check, one repository and those created since a time.
pub fn failures(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
    check: Option<&str>,
    repository: Option<&str>,
    since: Option<&str>,
) -> Result<Value, WgError> {
    require(app, context, Permission::TaskRead)?;
    let ws = app.workspace_or_default(Some(workspace))?;
    let since = since.map(parse_since).transpose()?;
    app.runtime.store.read(|session| {
        let repository_id = match repository {
            Some(reference) => {
                let registry = session.registry_world(Some(ws))?;
                Some(resolve::repository(&registry, ws, reference, false)?.id)
            }
            None => None,
        };
        let filter = FailureFilter {
            check_name: check.map(str::to_owned),
            repository_id,
            since,
        };
        let page = Page {
            limit: FAILURE_LIMIT,
            offset: 0,
        };
        let rows = reads::verification_failures(session, ws, &filter, page)?;
        Ok(Value::List(
            rows.iter()
                .map(|row| {
                    failure_row(
                        &row.check,
                        row.run_id,
                        row.task_id,
                        row.attempt_id,
                        &row.task_key.to_string(),
                    )
                })
                .collect(),
        ))
    })
}
