//! Shell: the verification listings the `verify` commands print (`VerificationService.list_configs`,
//! `list_runs` and `failures`). Each returns the wire documents of its rows, as the Python service
//! returns them to `emit`. A workspace the caller does not name is the default one.

use wg_json::Value;
use wg_model::entity::VerificationRun;
use wg_registry::resolve;
use wg_runtime::App;
use wg_store::Session;
use wg_types::{Id, WgError};
use wg_wire::dto::verification::{config_from_model, failure_row, run_from_model};

use crate::service::reads;
use crate::spec::sql::{FailureFilter, Page, RunFilter};

/// The `limit` of `list_runs` (the CLI does not pass one).
const RUN_LIMIT: i64 = 50;

/// The `limit` of `failures` (the CLI does not pass one).
const FAILURE_LIMIT: i64 = 200;

/// The workspace `reference` names, else the default workspace (`_workspace_id`).
fn workspace_or_default(app: &mut App, reference: Option<&str>) -> Result<Id, WgError> {
    match reference {
        Some(text) => app.workspace_id(text),
        None => app.default_workspace_id(),
    }
}

/// `list_configs`: the configs of the workspace, by scope type and name.
pub fn configs(app: &mut App, workspace: Option<&str>) -> Result<Value, WgError> {
    let ws = workspace_or_default(app, workspace)?;
    let configs = app
        .runtime
        .store
        .read(|session| session.verification_configs(ws))?;
    Ok(Value::List(configs.iter().map(config_from_model).collect()))
}

/// `list_runs` with the CLI's filter: the newest runs of the workspace, optionally only those of
/// one task. Each run carries its manifest entries and its check results.
pub fn runs(app: &mut App, workspace: Option<&str>, task: Option<&str>) -> Result<Value, WgError> {
    let ws = workspace_or_default(app, workspace)?;
    app.runtime.store.read(|session| {
        let task_id = match task {
            Some(reference) => Some(resolve_task(session, ws, reference)?),
            None => None,
        };
        let filter = RunFilter {
            task_id,
            attempt_id: None,
            status: None,
        };
        let page = Page {
            limit: RUN_LIMIT,
            offset: 0,
        };
        let runs = reads::verification_runs(session, ws, &filter, page)?;
        let documents = runs
            .iter()
            .map(|run| run_document(session, run))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Value::List(documents))
    })
}

/// `failures`: the failed and errored check results of the workspace, newest first, optionally
/// only those of checks named `check`.
pub fn failures(
    app: &mut App,
    workspace: Option<&str>,
    check: Option<&str>,
) -> Result<Value, WgError> {
    let ws = workspace_or_default(app, workspace)?;
    let filter = FailureFilter {
        check_name: check.map(str::to_owned),
        repository_id: None,
        since: None,
    };
    let page = Page {
        limit: FAILURE_LIMIT,
        offset: 0,
    };
    let rows = app
        .runtime
        .store
        .read(|session| reads::verification_failures(session, ws, &filter, page))?;
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
}

/// The task `reference` names in the workspace (`resolve.task`).
fn resolve_task(session: &mut Session<'_>, ws: Id, reference: &str) -> Result<Id, WgError> {
    let world = session.task_world(Some(ws), &[])?;
    Ok(resolve::task(&world.tasks, Some(ws), reference)?.id)
}

/// One run as `VerificationRunOut`: its manifest entries and its check results.
pub(crate) fn run_document(
    session: &mut Session<'_>,
    run: &VerificationRun,
) -> Result<Value, WgError> {
    let manifest = match run.manifest_id {
        Some(id) => session
            .manifest_by_id(id)?
            .map(|found| found.entries.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let checks = reads::verification_check_results(session, run.id)?;
    Ok(run_from_model(run, manifest.iter(), checks.iter()))
}
