//! `workgraph verify configs, runs, failures` (IF-10, C173, C175-C176 listing part of C172-C176).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`.
//! `verify config` (create or update a config) and `verify run` (run the checks) are not ported:
//! the first needs the project and repository scope resolution of the registry and its write, the
//! second needs the check runner and the revision manifests of the run flow. Both answer the
//! placeholder message until they are.

use clap::ArgMatches;
use wg_clispec::args::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState};

use crate::service::listing;
use crate::spec::cli as view;

/// The `verify` group.
pub const GROUP: CliGroup = CliGroup {
    name: "verify",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    match path.get(1).map(String::as_str) {
        Some("configs") => {
            let out = listing::configs(state.app()?, ws)?;
            state.emit(&out, Some(&|c| view::configs_view(c, &out)))
        }
        Some("runs") => {
            let task = option_text(m, "--task");
            let out = listing::runs(state.app()?, ws, task.as_deref())?;
            state.emit(&out, Some(&|c| view::runs_view(c, &out)))
        }
        Some("failures") => {
            let check = option_text(m, "--check");
            let out = listing::failures(state.app()?, ws, check.as_deref())?;
            state.emit(&out, Some(&|c| view::failures_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
