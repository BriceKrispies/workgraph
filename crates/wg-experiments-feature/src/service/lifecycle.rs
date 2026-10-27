//! The experiment lifecycle entry points of the application (IF-19): `experiment create`,
//! `freeze` and `close`. Each checks the write permission, resolves the workspace, loads what the
//! decision needs in its transaction and applies what the decision returns.

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, WgError};
use wg_wire::dto::experiment;

use super::experiments::find;
use super::reads;
use crate::spec::lifecycle;

/// `ExperimentService.create`: a draft experiment with a validated protocol.
pub fn experiment_create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    slug: &str,
    title: &str,
    question: Option<&str>,
    protocol: &Json,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let checked = lifecycle::check_create(slug, title, protocol)?;
    let scope = app.workspace_or_default(workspace)?;
    let row = app
        .runtime
        .decide(
            context,
            |session| reads::experiment_by_slug(session, scope, slug),
            |existing, stamp| {
                lifecycle::create(
                    stamp,
                    scope,
                    slug,
                    title,
                    question,
                    checked.clone(),
                    existing.is_some(),
                )
            },
        )?
        .reply;
    Ok(experiment::from_model(&row))
}

/// `ExperimentService.freeze`: validates the protocol, freezes it and starts the experiment.
pub fn experiment_freeze(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    let row = app
        .runtime
        .decide(
            context,
            |session| find(session, scope, reference),
            |found, stamp| lifecycle::freeze(stamp, found),
        )?
        .reply;
    Ok(experiment::from_model(&row))
}

/// `ExperimentService.close`: a running experiment stops taking trial changes.
pub fn experiment_close(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    let row = app
        .runtime
        .decide(
            context,
            |session| {
                let found = find(session, scope, reference)?;
                let trials = reads::trial_count(session, found.id)?;
                Ok((found, trials))
            },
            |(found, trials), stamp| lifecycle::close(stamp, found, *trials),
        )?
        .reply;
    Ok(experiment::from_model(&row))
}
