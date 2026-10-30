//! Pure: the arguments of the `claude` commands (`cli/commands/claude.py`): the readers of the
//! matched options, which hold the defaults and the Click usage lines the errors name. The
//! handlers in [`crate::cli`] run them; the clap command itself is the snapshot's own tree.

use std::path::PathBuf;

use clap::{ArgMatches, Command};
use wg_iface::cli::args::{number, one};
use wg_iface::cli::{CliFailure, Usage};

/// The clap command of the `claude` group: the snapshot's own tree, so help and parameters match
/// `contracts/cli.json`.
#[must_use]
pub fn command() -> Command {
    wg_iface::cli::tree::snapshot_command("claude")
}

/// The usage line of `claude status`.
pub const STATUS: Usage = Usage {
    usage: "workgraph claude status [OPTIONS]",
    command: "workgraph claude status",
};

/// The usage line of `claude replay`.
pub const REPLAY: Usage = Usage {
    usage: "workgraph claude replay [OPTIONS]",
    command: "workgraph claude replay",
};

/// The usage line of `claude backfill-time`.
pub const BACKFILL_TIME: Usage = Usage {
    usage: "workgraph claude backfill-time [OPTIONS]",
    command: "workgraph claude backfill-time",
};

/// The usage line of `claude backfill-transcripts`.
pub const BACKFILL_TRANSCRIPTS: Usage = Usage {
    usage: "workgraph claude backfill-transcripts [OPTIONS]",
    command: "workgraph claude backfill-transcripts",
};

/// The usage line of `claude bind`.
pub const BIND: Usage = Usage {
    usage: "workgraph claude bind [OPTIONS] {task}",
    command: "workgraph claude bind",
};

/// The usage line of `claude unbind`.
pub const UNBIND: Usage = Usage {
    usage: "workgraph claude unbind [OPTIONS]",
    command: "workgraph claude unbind",
};

/// The usage line of `claude sessions`.
pub const SESSIONS: Usage = Usage {
    usage: "workgraph claude sessions [OPTIONS]",
    command: "workgraph claude sessions",
};

/// The usage line of `claude mcp-config`.
pub const MCP_CONFIG: Usage = Usage {
    usage: "workgraph claude mcp-config [OPTIONS]",
    command: "workgraph claude mcp-config",
};

/// `--scope` of `status`: `user` unless the option is given (an empty value is kept, as Python
/// keeps it).
#[must_use]
pub fn scope(m: &ArgMatches) -> String {
    one(m, "--scope").unwrap_or_else(|| "user".to_owned())
}

/// `--project-dir`, when given.
#[must_use]
pub fn project_dir(m: &ArgMatches) -> Option<PathBuf> {
    one(m, "--project-dir").map(PathBuf::from)
}

/// `--cwd`, when given and not empty (Python's `if cwd` test).
#[must_use]
pub fn cwd(m: &ArgMatches) -> Option<PathBuf> {
    one(m, "--cwd")
        .filter(|text| !text.is_empty())
        .map(PathBuf::from)
}

/// The `TASK` argument of `bind`.
#[must_use]
pub fn task(m: &ArgMatches) -> String {
    one(m, "task").unwrap_or_default()
}

/// An option whose value is text, when it is given and not empty (Python's truthiness test).
#[must_use]
pub fn text(m: &ArgMatches, id: &str) -> Option<String> {
    one(m, id).filter(|text| !text.is_empty())
}

/// An `int` option with a default (`--limit` of `replay` and `sessions`).
pub fn limit(m: &ArgMatches, default: i64, usage: Usage) -> Result<i64, CliFailure> {
    number(m, "--limit", default, usage)
}

/// An `int` option with no default (`--limit` of the backfills): `None` when it is not given.
pub fn optional_limit(m: &ArgMatches, usage: Usage) -> Result<Option<i64>, CliFailure> {
    match one(m, "--limit") {
        None => Ok(None),
        Some(_) => number(m, "--limit", 0, usage).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_lines_name_their_commands() {
        assert_eq!(BIND.usage, "workgraph claude bind [OPTIONS] {task}");
        assert_eq!(MCP_CONFIG.command, "workgraph claude mcp-config");
    }
}
