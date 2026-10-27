//! `workgraph agent list, add-command` (IF-09). The clap command is the snapshot's own tree.

use clap::ArgMatches;
use wg_clispec::args::{option_flag, option_list, option_text};
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::agent_cmds::{self as agents, CommandAgent};
use crate::spec::cli_agent::{command, list_view};

/// The `agent` group.
pub const GROUP: CliGroup = CliGroup {
    name: "agent",
    command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let out = agents::agent_list(state.app()?)?;
            state.emit(&out, Some(&|c| list_view(c, &out)))
        }
        Some("add-command") => {
            human_context(None)?;
            let name = option_text(m, "name").unwrap_or_default();
            let executable = option_text(m, "executable").unwrap_or_default();
            let args = option_list(m, "--arg");
            let timeout = match option_text(m, "--timeout") {
                Some(text) => text
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| CliFailure::fail(format!("{text} is not a valid float"), 2))?,
                None => 1800.0,
            };
            let supports_usage = option_flag(m, "--supports-usage");
            let agent = CommandAgent {
                name: &name,
                executable: &executable,
                args: &args,
                timeout_seconds: timeout,
                supports_usage,
            };
            let out = agents::agent_add_command(state.app()?, &agent)?;
            state.emit(
                &out,
                Some(&|c| c.print(&format!("agent {name} configured")).text),
            )
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
