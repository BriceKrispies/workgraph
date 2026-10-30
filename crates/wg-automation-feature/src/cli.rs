//! `workgraph automation list, create, update, show, enable, disable, trigger, preview, runs,
//! notifications, catalog, process` (C025-C036): the declarative rules, their dry runs and history.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads its matches with [`crate::spec::args`], calls one [`crate::service`] entry
//! point and prints the result with [`crate::spec::view`].

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::option_text;
use wg_runtime::{CliState, human_context, system_context};

use crate::service;
use crate::spec::{args, catalog, view};

/// The `automation` group.
pub const GROUP: wg_runtime::CliGroup = wg_runtime::CliGroup {
    name: "automation",
    command: args::command,
    run,
};

/// Reads a file for a `@path` argument.
fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let context = human_context(None)?;
            let out = service::list(state.app()?, &context, ws)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("create") => {
            let context = human_context(None)?;
            let spec = args::rule_document(m, "spec", &read_file)?;
            let out = service::create(state.app()?, &context, ws, &spec)?;
            state.emit(&out, Some(&|c| view::created_line(c, &out)))
        }
        Some("update") => {
            let context = human_context(None)?;
            let name = args::name_arg(m);
            let spec = args::rule_document(m, "spec", &read_file)?;
            let out = service::update(state.app()?, &context, ws, &name, &spec)?;
            state.emit(&out, Some(&|c| view::updated_line(c, &out)))
        }
        Some("show") => {
            let context = human_context(None)?;
            let name = args::name_arg(m);
            let out = service::get(state.app()?, &context, ws, &name)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some(action @ ("enable" | "disable")) => {
            let context = human_context(None)?;
            let name = args::name_arg(m);
            let enabled = action == "enable";
            let out = service::set_enabled(state.app()?, &context, ws, &name, enabled)?;
            state.emit(&out, Some(&|c| view::toggled_line(c, &out, enabled)))
        }
        Some("trigger") => {
            let context = human_context(None)?;
            let name = args::name_arg(m);
            let inputs = args::trigger_inputs(m, &read_file)?;
            let out = service::trigger(state.app()?, &context, ws, &name, &inputs)?;
            state.emit(&out, Some(&|c| view::trigger_line(c, &out)))
        }
        Some("preview") => {
            let context = human_context(None)?;
            let (rule, event) = args::preview_targets(m);
            let spec = args::preview_spec(m, &read_file)?;
            let inputs = args::preview_inputs(m);
            let out = service::preview(
                state.app()?,
                &context,
                ws,
                rule.as_deref(),
                spec.as_ref(),
                event.as_deref(),
                &inputs,
            )?;
            state.emit(&out, Some(&|c| view::preview_view(c, &out)))
        }
        Some("runs") => {
            let context = human_context(None)?;
            let (rule, status, limit) = args::runs_args(m)?;
            let out = service::runs(
                state.app()?,
                &context,
                ws,
                rule.as_deref(),
                status.as_deref(),
                limit,
            )?;
            state.emit(&out, Some(&|c| view::runs_view(c, &out)))
        }
        Some("notifications") => {
            let context = human_context(None)?;
            let out = service::notifications(state.app()?, &context, ws, args::unread_only(m))?;
            state.emit(&out, Some(&|c| view::notifications_view(c, &out)))
        }
        Some("catalog") => {
            state.app()?;
            let out = catalog();
            state.emit(&out, Some(&|c| view::catalog_view(c, &out)))
        }
        Some("process") => {
            let context = system_context("cli")?;
            let out = service::process(state.app()?, &context)?;
            state.emit(&out, Some(&|c| view::process_view(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_iface::cli::not_implemented_message(path),
            1,
        )),
    }
}
