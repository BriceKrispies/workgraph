//! `workgraph run get, watch` (IF-10, C127 and C132).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`.
//! `run start`, `cancel`, `retry`, `rerun`, `schedule` and `decompose` are not ported: each needs
//! the run flow's writes (queued agent jobs, adapter launches, cancellation of a running job, the
//! scheduler pass, the child proposal), which the exec feature does not serve yet. They answer the
//! placeholder message until they are.

use std::time::{Duration, Instant};

use clap::ArgMatches;
use wg_clispec::args::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState};

use crate::service::runs;
use crate::spec::cli_run as view;

/// How many recent events `run get` and `run watch` show.
const RECENT_EVENTS: i64 = 50;

/// How long `run watch` waits for the attempt to finish (`_wait`'s default timeout).
const WATCH_TIMEOUT: Duration = Duration::from_secs(7200);

/// The interval between two reads of `run watch`.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// The `run` group.
pub const GROUP: CliGroup = CliGroup {
    name: "run",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    // `get` and `watch` take only the attempt, a positional argument.
    let attempt = option_text(m, "attempt_id").unwrap_or_default();
    match path.get(1).map(String::as_str) {
        Some("get") => show(state, &attempt),
        Some("watch") => {
            wait(state, &attempt)?;
            show(state, &attempt)
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `run get`: the attempt, its job and its recent events.
fn show(state: &mut CliState, attempt: &str) -> Result<(), CliFailure> {
    let out = runs::get(state.app()?, attempt, RECENT_EVENTS)?;
    state.emit(&out, Some(&|c| view::get_view(c, &out)))
}

/// `_wait`: polls the attempt until it finishes, printing each new progress event as it arrives
/// (human output only). Fails with exit code 3 when the attempt does not finish in time.
fn wait(state: &mut CliState, attempt: &str) -> Result<(), CliFailure> {
    let deadline = Instant::now() + WATCH_TIMEOUT;
    let mut last_seq = 0;
    while Instant::now() < deadline {
        let run = runs::get(state.app()?, attempt, RECENT_EVENTS)?;
        let mut progress = String::new();
        for event in wg_clispec::args::items(wg_clispec::args::get(&run, "recent_events")) {
            let seq = view::seq_of(event);
            if seq > last_seq {
                last_seq = seq;
                if !state.json {
                    progress.push_str(&state.console.print(&view::progress_markup(event)).text);
                }
            }
        }
        state.print(&progress);
        if view::finished(&run) {
            return Ok(());
        }
        std::thread::sleep(WATCH_INTERVAL);
    }
    Err(CliFailure::fail("timed out waiting for the run", 3))
}
