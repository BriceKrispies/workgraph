//! The clap shape and argument reading of `workgraph serve` and `workgraph mcp`
//! (`cli/commands/servers.py`): the commands are the snapshot's own.

use clap::{ArgMatches, Command};

use crate::CliFailure;
use crate::task::{option_flag, option_list, option_text};
use crate::tree::snapshot_command;

/// The clap command of `serve`.
#[must_use]
pub fn serve_command() -> Command {
    snapshot_command("serve")
}

/// The clap command of `mcp`.
#[must_use]
pub fn mcp_command() -> Command {
    snapshot_command("mcp")
}

/// `--port`, when given: a number in range, else the usage error (exit 2).
///
/// # Errors
/// A usage failure for a value that is not a port.
pub fn port_option(m: &ArgMatches) -> Result<Option<u16>, CliFailure> {
    option_text(m, "--port")
        .map(|text| {
            text.parse::<u16>().map_err(|_| {
                CliFailure::fail(
                    format!("Invalid value for '--port': '{text}' is not a valid port"),
                    2,
                )
            })
        })
        .transpose()
}

/// A `--name/--no-name` pair left unset (`None`), on or off; the twin's id is the name with
/// `__secondary`, and the last one typed wins because the twins override each other.
#[must_use]
pub fn pair(m: &ArgMatches, name: &str) -> Option<bool> {
    match (
        option_flag(m, name),
        option_flag(m, &format!("{name}__secondary")),
    ) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// The flags of `mcp` as the argument list `wg_mcpspec::options::ServeOptions::parse` reads, so
/// the binary and the standalone `wg-mcp` validate alike.
#[must_use]
pub fn mcp_arguments(m: &ArgMatches) -> Vec<String> {
    let single = |flag: &str| {
        option_text(m, flag)
            .into_iter()
            .flat_map(|value| [flag.to_owned(), value])
            .collect::<Vec<_>>()
    };
    let grants = option_list(m, "--grant")
        .into_iter()
        .flat_map(|grant| ["--grant".to_owned(), grant]);
    single("--transport")
        .into_iter()
        .chain(single("--agent-name"))
        .chain(grants)
        .chain(single("--host"))
        .chain(single("--port"))
        .collect()
}
