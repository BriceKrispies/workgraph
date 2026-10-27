//! `workgraph analytics metrics, query, overview, compare, coupling, failures, history`
//! (`cli/commands/analytics.py`): the engineering metrics.
//!
//! The clap tree and the readers of the matched options are in [`crate::spec::args`]; each
//! implemented leaf calls one [`crate::service`] entry point (or a pure [`crate::spec`] decision)
//! and prints the result with [`crate::spec::view`]. The rest of the group is the snapshot's
//! placeholder and reports itself as not implemented.

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_clispec::args::option_text;
use wg_runtime::{CliState, human_context};

use crate::service;
use crate::spec::{args, query, view};

/// The `analytics` group.
pub const GROUP: wg_runtime::CliGroup = wg_runtime::CliGroup {
    name: "analytics",
    command: args::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    match path.get(1).map(String::as_str) {
        Some("metrics") => {
            state.app()?;
            let out = query::definitions();
            state.emit(&out, Some(&|c| view::metrics_view(c, &out)))
        }
        Some("query") => {
            let context = human_context(None)?;
            let request = args::query_request(m)?;
            let out = service::query(state.app()?, &context, ws, &request)?;
            state.emit(&out, Some(&|c| view::query_view(c, &out)))
        }
        Some("overview") => {
            let context = human_context(None)?;
            let period = args::overview_period(m);
            let out = service::overview(state.app()?, &context, ws, &period)?;
            state.emit(&out, Some(&|c| view::overview_view(c, &out)))
        }
        Some("compare") => {
            let context = human_context(None)?;
            let (metric, first, second, group_by) = args::compare_arguments(m)?;
            let out = service::compare(
                state.app()?,
                &context,
                ws,
                &metric,
                first,
                second,
                &group_by,
            )?;
            state.emit(&out, Some(&|c| view::compare_view(c, &out, &group_by)))
        }
        Some("coupling") => {
            let context = human_context(None)?;
            let (project, min_support) = args::coupling_options(m)?;
            let out =
                service::coupling(state.app()?, &context, ws, project.as_deref(), min_support)?;
            state.emit(&out, Some(&|c| view::coupling_view(c, &out)))
        }
        Some("failures") => {
            let context = human_context(None)?;
            let persist = args::failures_persist(m);
            let out = service::failures(state.app()?, &context, ws, persist)?;
            state.emit(&out, Some(&|c| view::failures_view(c, &out)))
        }
        Some("history") => {
            let context = human_context(None)?;
            let (category, repository, limit) = args::history_options(m)?;
            let out = service::history(
                state.app()?,
                &context,
                ws,
                category.as_deref(),
                repository.as_deref(),
                limit,
            )?;
            state.emit(&out, Some(&|c| view::history_view(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_clispec::not_implemented_message(path),
            1,
        )),
    }
}
