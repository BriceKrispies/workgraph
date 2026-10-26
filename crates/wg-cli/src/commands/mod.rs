//! The command groups implemented so far, one module per group (`commands/<group>.rs`).
//!
//! A group task adds its module, appends a [`Group`] to [`GROUPS`] and adds the group's name to
//! the implemented list of `tests/if01_cli.rs`, which then compares its clap command against
//! `contracts/cli.json`. A top-level command or group that is not listed here is a placeholder
//! built from the snapshot (see `wg_clispec::tree`).

pub mod setup;
pub mod task;

use clap::{ArgMatches, Command};
use wg_clispec::CliFailure;

use crate::CliState;

/// What a handler receives: the global state, the command path below the root
/// (`["task", "list"]`) and the matches of the leaf command.
pub type Handler = fn(&mut CliState, &[String], &ArgMatches) -> Result<(), CliFailure>;

/// One top-level command or group.
pub struct Group {
    /// The name as typed after `workgraph`.
    pub name: &'static str,
    /// Builds the clap command; start from [`wg_clispec::tree::command`] so the help flag matches.
    pub command: fn() -> Command,
    /// Runs the matched leaf command of this group.
    pub run: Handler,
}

/// The implemented groups.
pub const GROUPS: &[Group] = &[setup::CONFIG, setup::DOCTOR, setup::INIT, task::GROUP];
