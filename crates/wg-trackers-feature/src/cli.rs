//! `workgraph tracker list, status, sync` (C168 to C170): the configured connections, their sync
//! status, and the sync of a connection (queued with `--queue`).
//!
//! The clap command is the snapshot's own tree (see [`crate::spec::args`]); each handler reads its
//! matches, calls one [`crate::service`] entry point and prints the result with
//! [`crate::spec::view`].

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_runtime::{CliState, human_context};

use crate::service;
use crate::spec::{args, status_document, view};

/// The `tracker` group.
pub const GROUP: wg_runtime::CliGroup = wg_runtime::CliGroup {
    name: "tracker",
    command: args::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let rows = service::list(state.app()?)?;
            let out = crate::spec::list_out(&rows);
            state.emit(&out, Some(&|c| view::list_view(c, &rows)))
        }
        Some("status") => {
            // `if connection:` in Python: an empty name lists every connection.
            let connection = args::connection(m).filter(|name| !name.is_empty());
            let statuses = service::status(state.app()?, connection.as_deref())?;
            let out = status_document(&statuses, connection.is_some());
            state.emit(&out, Some(&|c| view::status_view(c, &statuses)))
        }
        Some("sync") => {
            let connection = args::connection(m).unwrap_or_default();
            if args::queue(m) {
                let full = args::full(m);
                let context = human_context(None)?;
                let job_id = service::enqueue_sync(state.app()?, &context, &connection, full)?;
                let job = job_id.to_string();
                let out = crate::spec::enqueued_out(&job, &connection, full);
                state.emit(&out, Some(&|c| view::enqueued_line(c, &job)))
            } else {
                let full = args::full(m);
                let context = human_context(None)?;
                let report = service::sync::run_now(state.app()?, &context, &connection, full)?;
                let out = crate::spec::run::sync_out(&report);
                state.emit(&out, Some(&|c| view::sync_view(c, &report)))
            }
        }
        _ => Err(CliFailure::fail(
            wg_clispec::not_implemented_message(path),
            1,
        )),
    }
}
