//! `workgraph verify config, configs, run, runs, failures` (IF-10, C172-C176).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`.
//! Each handler reads its arguments with [`crate::spec::cli`], calls one [`crate::service`] entry
//! point and prints the result.

use clap::ArgMatches;
use wg_iface::cli::args::option_text;
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::{config, listing, run as runner};
use crate::spec::cli as view;
use crate::spec::config_write::ConfigRequest;

/// The `verify` group.
pub const GROUP: CliGroup = CliGroup {
    name: "verify",
    command: view::command,
    run,
};

fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| error.to_string())
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    match path.get(1).map(String::as_str) {
        Some("config") => {
            let args = view::config_args(m, &read_file)?;
            let request = ConfigRequest {
                scope_type: &args.scope_type,
                scope: args.scope.as_deref(),
                name: &args.name,
                checks: &args.checks,
            };
            let out = config::config(state.app()?, &human_context(None)?, ws, &request)?;
            state.emit(&out, Some(&|c| view::config_line(c, &out)))
        }
        Some("configs") => {
            let out = listing::configs(state.app()?, ws)?;
            state.emit(&out, Some(&|c| view::configs_view(c, &out)))
        }
        Some("run") => {
            let tier = wg_verify::parse_tier(option_text(m, "--tier").as_deref())?;
            let task = option_text(m, "task").unwrap_or_default();
            let attempt = option_text(m, "--attempt");
            let input = runner::RunInput {
                workspace: ws,
                task: &task,
                attempt: attempt.as_deref(),
                tier,
            };
            let out = runner::run(state.app()?, &human_context(None)?, &input, None)?;
            state.emit(&out, Some(&|c| view::run_view(c, &out)))?;
            if view::run_passed(&out) {
                Ok(())
            } else {
                Err(CliFailure::Exit(3))
            }
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
