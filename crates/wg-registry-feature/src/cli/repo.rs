//! `workgraph repo register, list, show, refresh, status, update, remove, remove-checkout,
//! depend` (IF-04).
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler builds its input with [`crate::spec::cli_repo`], calls one
//! [`crate::service`] entry point and prints the result.

use clap::ArgMatches;
use wg_iface::cli::args::option_text;
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_json::Value;
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service;
use crate::spec::cli_repo as view;

/// The `repo` group.
pub const GROUP: CliGroup = CliGroup {
    name: "repo",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    let reference = option_text(m, "ref").unwrap_or_default();
    let out = match path.get(1).map(String::as_str) {
        Some("register") => {
            service::repo_register(state.app()?, &context, ws, &view::register_input(m))?
        }
        Some("list") => {
            let (all, project) = view::list_scope(m);
            let items = service::repo_list(state.app()?, &context, ws, all, project.as_deref())?;
            let out = Value::List(items);
            return state.emit(&out, Some(&|c| view::list_view(c, &out)));
        }
        Some("show") => service::repo_get(state.app()?, &context, ws, &reference)?,
        Some("refresh") => service::repo_refresh(state.app()?, &context, ws, &reference)?,
        Some("status") => {
            let out = service::repo_status(state.app()?, &context, ws, &reference)?;
            return state.emit(&out, Some(&|c| view::status_view(c, &out)));
        }
        Some("update") => update(state, &context, ws, &reference, m)?,
        Some("remove") => {
            let out = service::repo_remove(state.app()?, &context, ws, &reference)?;
            return state.emit(&out, Some(&|c| view::removed_line(c, &out)));
        }
        Some("remove-checkout") => {
            let checkout = option_text(m, "path").unwrap_or_default();
            service::repo_remove_checkout(state.app()?, &context, ws, &reference, &checkout)?
        }
        Some("depend") => {
            let (other, description, remove) = view::depend_args(m);
            service::repo_depend(
                state.app()?,
                &context,
                ws,
                &reference,
                &other,
                &description,
                remove,
            )?
        }
        _ => return Err(CliFailure::fail(not_implemented_message(path), 1)),
    };
    state.emit(&out, Some(&|c| view::repo_view(c, &out)))
}

/// `repo update`: `--track` and `--untrack` edit the current list, read first.
fn update(
    state: &mut CliState,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    reference: &str,
    m: &ArgMatches,
) -> Result<Value, CliFailure> {
    let (track, untrack) = view::track_options(m);
    let tracked = if track.is_empty() && untrack.is_empty() {
        None
    } else {
        let current = service::repo_get(state.app()?, context, ws, reference)?;
        view::tracked_after(&view::tracked_of(&current), &track, &untrack)
    };
    let patch = view::update_patch(m, tracked)?;
    Ok(service::repo_update(
        state.app()?,
        context,
        ws,
        reference,
        &patch,
    )?)
}
