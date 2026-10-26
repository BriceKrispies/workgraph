//! `workgraph automation list, create, update, show, enable, disable, trigger, preview, runs,
//! notifications, catalog, process` (C025-C036): the declarative rules, their dry runs and history.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads its matches, calls one `App` entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_clispec::args::option_text;
use wg_clispec::automation as view;
use wg_runtime::{human_context, system_context};

use super::Group;
use crate::CliState;

/// The `automation` group.
pub const GROUP: Group = Group {
    name: "automation",
    command: view::command,
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
            let out = state.app()?.automation_list(&context, ws)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("create") => {
            let context = human_context(None)?;
            let spec = view::rule_document(m, "spec", &read_file)?;
            let out = state.app()?.automation_create(&context, ws, &spec)?;
            state.emit(&out, Some(&|c| view::created_line(c, &out)))
        }
        Some("update") => {
            let context = human_context(None)?;
            let name = view::name_arg(m);
            let spec = view::rule_document(m, "spec", &read_file)?;
            let out = state.app()?.automation_update(&context, ws, &name, &spec)?;
            state.emit(&out, Some(&|c| view::updated_line(c, &out)))
        }
        Some("show") => {
            let context = human_context(None)?;
            let name = view::name_arg(m);
            let out = state.app()?.automation_get(&context, ws, &name)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some(action @ ("enable" | "disable")) => {
            let context = human_context(None)?;
            let name = view::name_arg(m);
            let enabled = action == "enable";
            let out = state
                .app()?
                .automation_set_enabled(&context, ws, &name, enabled)?;
            state.emit(&out, Some(&|c| view::toggled_line(c, &out, enabled)))
        }
        Some("trigger") => {
            let context = human_context(None)?;
            let name = view::name_arg(m);
            let inputs = view::trigger_inputs(m, &read_file)?;
            let out = state
                .app()?
                .automation_trigger(&context, ws, &name, &inputs)?;
            state.emit(&out, Some(&|c| view::trigger_line(c, &out)))
        }
        Some("preview") => {
            let context = human_context(None)?;
            let (rule, event) = view::preview_targets(m);
            let spec = view::preview_spec(m, &read_file)?;
            let inputs = view::preview_inputs(m);
            let out = state.app()?.automation_preview(
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
            let (rule, status, limit) = view::runs_args(m)?;
            let out = state.app()?.automation_runs(
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
            let out = state
                .app()?
                .automation_notifications(&context, ws, view::unread_only(m))?;
            state.emit(&out, Some(&|c| view::notifications_view(c, &out)))
        }
        Some("catalog") => {
            state.app()?;
            let out = wg_runtime::automation_catalog();
            state.emit(&out, Some(&|c| view::catalog_view(c, &out)))
        }
        Some("process") => {
            let context = system_context("cli")?;
            let out = state.app()?.automation_process(&context)?;
            state.emit(&out, Some(&|c| view::process_view(c, &out)))
        }
        _ => Err(CliFailure::fail(
            wg_clispec::not_implemented_message(path),
            1,
        )),
    }
}
