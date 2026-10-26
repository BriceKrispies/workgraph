//! The Pure half of the stacks feature: the clap tree of the `stack` group and the human views of
//! its results. The views are in [`view`].

pub mod view;

use wg_clispec::Snapshot;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};

/// The `stack` group's clap command, built from the snapshot (`contracts/cli.json`), so its
/// parameters and help match the Python CLI.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "stack"),
        Err(_) => wg_clispec::tree::command("stack"),
    }
}
