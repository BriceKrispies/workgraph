//! The arguments of the `tracker` commands (`cli/commands/tracker.py`): the clap command, which is
//! the snapshot's own tree so help and parameters match `contracts/cli.json`, and the readers of
//! the matched arguments and options.

use clap::ArgMatches;
use wg_clispec::Snapshot;
use wg_clispec::args::{flag, one};
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};

/// The clap command of the `tracker` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "tracker"),
        Err(_) => wg_clispec::tree::command("tracker"),
    }
}

/// The `connection` argument, when one was given.
#[must_use]
pub fn connection(m: &ArgMatches) -> Option<String> {
    one(m, "connection")
}

/// `--full`: every issue in scope, not only those changed since the cursor.
#[must_use]
pub fn full(m: &ArgMatches) -> bool {
    flag(m, "--full")
}

/// `--queue`: enqueue the sync as a job instead of running it now.
#[must_use]
pub fn queue(m: &ArgMatches) -> bool {
    flag(m, "--queue")
}
