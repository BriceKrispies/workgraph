//! The `decision` commands on the command line (`workgraph decision list, show, request, choose,
//! withdraw`): the handler. It reads the matched options with [`crate::spec::args`], calls one
//! [`crate::service`] entry point and prints the result with [`crate::spec::view`].

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_clispec::task::option_text;
use wg_runtime::{CliState, RequestContext, human_context};

use crate::service;
use crate::spec::args::{choose_args, command, decision_ref, list_args, reason, request_args};
use crate::spec::view;
use crate::spec::{DecisionFilter, filter_of};

/// The `decision` group.
pub const GROUP: wg_runtime::CliGroup = wg_runtime::CliGroup {
    name: "decision",
    command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let (status, limit) = list_args(m)?;
            let filter = filter_of(&status).unwrap_or(DecisionFilter::Open);
            let out = service::list(state.app()?, &context, ws, filter, limit)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("show") => {
            let id = full_id(state, &context, ws, m)?;
            let out = service::get(state.app()?, &context, ws, &id)?;
            state.emit(&out, Some(&|c| view::decision_view(c, &out)))
        }
        Some("request") => {
            let (task, question, options) = request_args(m)?;
            let out = service::request(state.app()?, &context, ws, &task, &question, &options)?;
            state.emit(&out, Some(&|c| view::requested_line(c, &out)))
        }
        Some("choose") => {
            let id = full_id(state, &context, ws, m)?;
            let [option, other, note] = choose_args(m);
            let out = service::choose(
                state.app()?,
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
            let reason = reason(m);
            let out = service::withdraw(state.app()?, &context, ws, &id, reason.as_deref())?;
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
    let reference = decision_ref(m);
    if view::is_full_id(&reference) {
        return Ok(reference);
    }
    let listed = service::list(
        state.app()?,
        context,
        ws,
        DecisionFilter::All,
        view::PREFIX_LOOKUP_LIMIT,
    )?;
    view::expand_prefix(&reference, &listed)
}
