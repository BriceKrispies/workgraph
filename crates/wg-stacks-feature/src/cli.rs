//! `workgraph stack create, show, list, add, remove, readmit, integrate, verify, eject, history`:
//! a parent task's children integrated as one unit.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads its matches, calls one [`crate::service`] entry point and prints the result
//! with [`crate::spec::view`].

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{one, option_flag, option_text};
use wg_runtime::{CliState, human_context};

use crate::service;
use crate::spec::{self, view};

/// The `stack` group.
pub const GROUP: wg_runtime::CliGroup = wg_runtime::CliGroup {
    name: "stack",
    command: spec::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let parent = one(m, "parent").unwrap_or_default();
    let task = one(m, "task").unwrap_or_default();
    match path.get(1).map(String::as_str) {
        Some("create") => {
            let context = human_context(None)?;
            let out = service::create(state.app()?, &context, ws, &parent)?;
            state.emit(&out, Some(&|c| view::stack_view(c, &out)))
        }
        Some("show") => {
            let context = human_context(None)?;
            let out = service::show(state.app()?, &context, ws, &parent)?;
            state.emit(&out, Some(&|c| view::stack_view(c, &out)))
        }
        Some("list") => {
            let context = human_context(None)?;
            let out = service::list(state.app()?, &context, ws, option_flag(m, "--all"))?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("add") => {
            let context = human_context(None)?;
            let out = service::add(state.app()?, &context, ws, &parent, &task)?;
            state.emit(&out, Some(&|c| view::stack_view(c, &out)))
        }
        Some("remove") => {
            let context = human_context(None)?;
            let out = service::remove(state.app()?, &context, ws, &parent, &task)?;
            state.emit(&out, Some(&|c| view::stack_view(c, &out)))
        }
        Some("readmit") => {
            let context = human_context(None)?;
            let out = service::readmit(state.app()?, &context, ws, &parent, &task)?;
            state.emit(&out, Some(&|c| view::stack_view(c, &out)))
        }
        Some("integrate") => {
            let context = human_context(None)?;
            let out = service::integrate(state.app()?, &context, ws, &parent)?;
            state.emit(&out, Some(&|c| view::integration_view(c, &out)))
        }
        Some("verify") => {
            let context = human_context(None)?;
            let out = service::verify(state.app()?, &context, ws, &parent)?;
            state.emit(&out, Some(&|c| view::verification_view(c, &out)))
        }
        Some("eject") => {
            let context = human_context(None)?;
            let reason = option_text(m, "--reason").unwrap_or_default();
            let out = service::exec::eject(state.app()?, &context, ws, &parent, &task, &reason)?;
            state.emit(&out, None)
        }
        Some("history") => {
            let context = human_context(None)?;
            let out = service::history(state.app()?, &context, ws, &parent)?;
            state.emit(&out, Some(&|c| view::history_view(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_iface::cli::not_implemented_message(path),
            1,
        )),
    }
}
