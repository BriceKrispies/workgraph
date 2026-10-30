//! `workgraph analytics metrics, query, overview, compare, coupling, failures, history, estimate,
//! retrospective, subagents, time, bottlenecks`
//! (`cli/commands/analytics.py`): the engineering metrics.
//!
//! The clap tree and the readers of the matched options are in [`crate::spec::args`]; each
//! implemented leaf calls one [`crate::service`] entry point (or a pure [`crate::spec`] decision)
//! and prints the result with [`crate::spec::view`]. The rest of the group is the snapshot's
//! placeholder and reports itself as not implemented.

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::option_text;
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
        Some("estimate") => {
            let context = human_context(None)?;
            let (task, title, category, repositories) = args::estimate_options(m);
            let out = service::estimate::estimate(
                state.app()?,
                &context,
                ws,
                task.as_deref(),
                title.as_deref(),
                "",
                category.as_deref(),
                &repositories,
            )?;
            state.emit(&out, Some(&|c| view::estimate_view(c, &out)))
        }
        Some("retrospective") => {
            let context = human_context(None)?;
            let options = args::retrospective_options(m)?;
            let out = service::retrospective::retrospective(state.app()?, &context, ws, &options)?;
            state.emit(&out, Some(&|c| view::retrospective_view(c, &out)))
        }
        Some("subagents") => {
            let context = human_context(None)?;
            let (period, since, until, model) = args::subagents_options(m)?;
            let out = service::subagents::report(
                state.app()?,
                &context,
                ws,
                &period,
                since,
                until,
                model.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::subagents_view(c, &out)))
        }
        Some("time") => {
            let context = human_context(None)?;
            let (since, until, by, model) = args::time_options(m)?;
            let out = service::subagents::breakdown(
                state.app()?,
                &context,
                ws,
                since,
                until,
                &by,
                model.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::agent_time_view(c, &out)))
        }
        Some("bottlenecks") => {
            let context = human_context(None)?;
            let (since, until, top) = args::bottleneck_options(m)?;
            let out =
                service::subagents::bottlenecks(state.app()?, &context, ws, since, until, top)?;
            state.emit(&out, Some(&|c| view::bottlenecks_view(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_iface::cli::not_implemented_message(path),
            1,
        )),
    }
}
