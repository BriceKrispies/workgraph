//! `workgraph job list, show, stats, cancel, retry, enqueue, compact` (IF-11): the durable job
//! queue.
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler reads its options with `crate::spec::job`, calls one `App`
//! entry point and prints the result.

use crate::spec::job as view;
use clap::ArgMatches;
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_runtime::{human_context, unknown_job_kind};

use super::Group;
use crate::CliState;

/// The `job` group.
pub const GROUP: Group = Group {
    name: "job",
    command: view::job_command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let (limit, offset) = view::page(m)?;
            let out = state.app()?.job_list(
                &view::statuses(m),
                view::kind_filter(m).as_deref(),
                limit,
                offset,
            )?;
            state.emit(&out, Some(&|c| view::job_list_view(c, &out)))
        }
        Some("show") => {
            let out = state.app()?.job_show(&view::job_id(m))?;
            state.emit(&out, Some(&|c| view::job_show_view(c, &out)))
        }
        Some("stats") => {
            let out = state.app()?.job_stats()?;
            state.emit(&out, Some(&|c| view::stats_view(c, &out)))
        }
        Some("cancel") => {
            let out = state.app()?.job_cancel(&context, &view::job_id(m))?;
            state.emit(&out, Some(&|c| view::cancelled_line(c, &out)))
        }
        Some("retry") => {
            let out = state.app()?.job_retry(&context, &view::job_id(m))?;
            state.emit(&out, Some(&|c| view::retried_line(c, &out)))
        }
        Some("enqueue") => {
            let (kind, payload, key) = view::enqueue_args(m);
            if let Some(message) = unknown_job_kind(&kind) {
                return Err(CliFailure::fail(message, 2));
            }
            let out = state
                .app()?
                .job_enqueue(&context, &kind, &payload, key.as_deref())?;
            state.emit(&out, Some(&|c| view::enqueued_line(c, &out)))
        }
        Some("compact") => {
            let out = state
                .app()?
                .job_compact(&context, view::compact_dry_run(m))?;
            state.emit(&out, Some(&|c| view::compact_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
