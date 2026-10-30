//! `workgraph demo e2e` and `demo seed` (C063, C064): the clap group comes from the snapshot, so
//! help and parameters match `contracts/cli.json`. Each handler reads its matches with
//! [`crate::spec`], calls one [`crate::service`] entry point and prints the result.

use clap::ArgMatches;
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState};

use crate::service;
use crate::spec;

/// The `demo` group.
pub const GROUP: CliGroup = CliGroup {
    name: "demo",
    command: spec::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("e2e") => e2e(state, m),
        Some("seed") => seed(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `demo e2e [--home DIR] [--keep]`: the demonstration in an isolated home. Its step lines go to
/// standard output unless `--json` is given, as `log=console.print` does in Python.
fn e2e(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let (home, keep) = spec::e2e_args(m);
    let temp = home.is_none();
    let home = match home {
        Some(home) => home,
        None => service::fresh_temp_home()?,
    };
    let mut lines = Vec::new();
    let result = service::e2e(&home, &mut lines);
    if !state.json {
        for line in &lines {
            state.print(&format!("{line}\n"));
        }
    }
    result?;
    if temp && !keep {
        let _ = std::fs::remove_dir_all(&home);
    }
    Ok(())
}

/// `demo seed [--workspace SLUG] [--force]`: the demo data for the web UI.
fn seed(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let (workspace, force) = spec::seed_args(m);
    let summary = service::seed(state.app()?, &workspace, force)?;
    state.emit(&summary, Some(&|c| spec::seed_view(c, &summary)))
}
