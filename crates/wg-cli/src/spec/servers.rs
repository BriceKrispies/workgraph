//! The clap shape and argument reading of `workgraph serve` and `workgraph mcp`
//! (`cli/commands/servers.py`): the commands are the snapshot's own.

use clap::{ArgMatches, Command};

use wg_iface::cli::args::{option_flag, option_list, option_text};
use wg_iface::cli::failure::python_number_text;
use wg_iface::cli::tree::snapshot_command;
use wg_iface::cli::{CliFailure, Usage};

/// The usage of `serve`, as Click prints it in a usage error.
const SERVE: Usage = Usage {
    usage: "workgraph serve [OPTIONS]",
    command: "workgraph serve",
};

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
            python_number_text(&text)
                .and_then(|digits| digits.parse::<u16>().ok())
                .ok_or_else(|| SERVE.invalid_value("--port", &text, "int"))
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

/// The flags of `mcp` as the argument list `wg_iface::mcp::options::ServeOptions::parse` reads, so
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
