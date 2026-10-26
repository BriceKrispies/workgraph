//! `workgraph decision list, show, request, choose, withdraw`: the questions tasks wait on, their
//! options and the answers.
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler builds its request with `wg_clispec::decision`, calls one
//! `App` entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_clispec::decision as view;
use wg_clispec::task::option_text;
use wg_runtime::{DecisionFilter, RequestContext, decision_filter, human_context};

use super::Group;
use crate::CliState;

/// The `decision` group.
pub const GROUP: Group = Group {
    name: "decision",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let (status, limit) = view::list_args(m)?;
            let filter = decision_filter(&status).unwrap_or(DecisionFilter::Open);
            let out = state.app()?.decision_list(&context, ws, filter, limit)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("show") => {
            let id = full_id(state, &context, ws, m)?;
            let out = state.app()?.decision_get(&context, ws, &id)?;
            state.emit(&out, Some(&|c| view::decision_view(c, &out)))
        }
        Some("request") => {
            let (task, question, options) = view::request_args(m)?;
            let out = state
                .app()?
                .decision_request(&context, ws, &task, &question, &options)?;
            state.emit(&out, Some(&|c| view::requested_line(c, &out)))
        }
        Some("choose") => {
            let id = full_id(state, &context, ws, m)?;
            let [option, other, note] = view::choose_args(m);
            let out = state.app()?.decision_choose(
                &context,
                ws,
                &id,
                option.as_deref(),
                other.as_deref(),
                note.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::chosen_line(c, &out)))
        }
        Some("withdraw") => {
            let id = full_id(state, &context, ws, m)?;
            let reason = view::reason(m);
            let out = state
                .app()?
                .decision_withdraw(&context, ws, &id, reason.as_deref())?;
            state.emit(&out, Some(&|c| view::withdrawn_line(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_clispec::not_implemented_message(path),
            1,
        )),
    }
}

/// `_full_id`: the 8-character prefix `decision list` prints, expanded to the full id.
fn full_id(
    state: &mut CliState,
    context: &RequestContext,
    ws: Option<&str>,
    m: &ArgMatches,
) -> Result<String, CliFailure> {
    let reference = view::decision_ref(m);
    if view::is_full_id(&reference) {
        return Ok(reference);
    }
    let listed =
        state
            .app()?
            .decision_list(context, ws, DecisionFilter::All, view::PREFIX_LOOKUP_LIMIT)?;
    view::expand_prefix(&reference, &listed)
}
