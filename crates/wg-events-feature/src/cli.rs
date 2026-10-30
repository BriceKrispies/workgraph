//! `workgraph events list` and `events verify` (C066, C067): the clap group comes from the snapshot,
//! so help and parameters match `contracts/cli.json`. Each handler reads its matches with
//! [`crate::spec`], calls one [`crate::service`] entry point and prints the result with
//! [`crate::spec`] (the `--json` document, or the human view).

use clap::ArgMatches;
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service;
use crate::spec;

/// The `events` group.
pub const GROUP: CliGroup = CliGroup {
    name: "events",
    command: spec::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("list") => list(state, m),
        Some("verify") => verify(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

fn list(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let args = spec::list_args(m)?;
    let workspace = state.workspace_ref(spec::workspace_arg(m).as_deref());
    let context = human_context(None)?;
    let events = service::list(state.app()?, &context, workspace.as_deref(), &args)?;
    let value = wg_json::Value::List(events);
    state.emit(&value, Some(&|c| spec::list_view(c, &value)))
}

fn verify(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(spec::workspace_arg(m).as_deref());
    let context = human_context(None)?;
    let report = service::verify(state.app()?, &context, workspace.as_deref())?;
    let value = spec::verify_document(&report.to_json());
    state.emit(&value, Some(&|c| spec::verify_view(c, &value)))?;
    if report.consistent() {
        Ok(())
    } else {
        Err(CliFailure::Exit(1))
    }
}
