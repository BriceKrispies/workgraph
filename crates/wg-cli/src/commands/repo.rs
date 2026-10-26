//! `workgraph repo register, list, show, refresh, status, update, remove, remove-checkout,
//! depend` (IF-04).
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler builds its input with `wg_clispec::repo`, calls one `App`
//! entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::repo as view;
use wg_clispec::task::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_json::Value;
use wg_runtime::human_context;

use super::Group;
use crate::CliState;

/// The `repo` group.
pub const GROUP: Group = Group {
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
        Some("register") => state
            .app()?
            .repo_register(&context, ws, &view::register_input(m))?,
        Some("list") => {
            let (all, project) = view::list_scope(m);
            let items = state
                .app()?
                .repo_list(&context, ws, all, project.as_deref())?;
            let out = Value::List(items);
            return state.emit(&out, Some(&|c| view::list_view(c, &out)));
        }
        Some("show") => state.app()?.repo_get(&context, ws, &reference)?,
        Some("refresh") => state.app()?.repo_refresh(&context, ws, &reference)?,
        Some("status") => {
            let out = state.app()?.repo_status(&context, ws, &reference)?;
            return state.emit(&out, Some(&|c| view::status_view(c, &out)));
        }
        Some("update") => update(state, &context, ws, &reference, m)?,
        Some("remove") => {
            let out = state.app()?.repo_remove(&context, ws, &reference)?;
            return state.emit(&out, Some(&|c| view::removed_line(c, &out)));
        }
        Some("remove-checkout") => {
            let checkout = option_text(m, "path").unwrap_or_default();
            state
                .app()?
                .repo_remove_checkout(&context, ws, &reference, &checkout)?
        }
        Some("depend") => {
            let (other, description, remove) = view::depend_args(m);
            state
                .app()?
                .repo_depend(&context, ws, &reference, &other, &description, remove)?
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
        let current = state.app()?.repo_get(context, ws, reference)?;
        view::tracked_after(&view::tracked_of(&current), &track, &untrack)
    };
    let patch = view::update_patch(m, tracked)?;
    Ok(state.app()?.repo_update(context, ws, reference, &patch)?)
}
