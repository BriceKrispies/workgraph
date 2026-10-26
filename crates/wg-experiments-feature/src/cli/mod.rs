//! `workgraph experiment list, show` (IF-19). The other `experiment` commands (`create`, `freeze`,
//! `close`, `next`, `report`, `fit`, `predict` and the `trial` and `replay` groups) are placeholders
//! until their services land.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler calls one service entry point and prints the result.

pub mod trial;

use crate::service;
use crate::spec::command;
use crate::spec::view::{list_view, show_view};
use clap::ArgMatches;
use wg_clispec::args::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

/// The `experiment` group.
pub const GROUP: CliGroup = CliGroup {
    name: "experiment",
    command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let status = option_text(m, "--status");
            let out = service::experiment_list(state.app()?, &context, ws, status.as_deref())?;
            state.emit(&out, Some(&|c| list_view(c, &out)))
        }
        Some("show") => {
            let reference = option_text(m, "experiment").unwrap_or_default();
            let out = service::experiment_show(state.app()?, &context, ws, &reference)?;
            state.emit(&out, Some(&|c| show_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
