//! `workgraph workspace create, list, show, use, archive, set` and `workgraph project create,
//! list, show, update, rename-term, add-repo, remove-repo` (IF-03).
//!
//! The clap commands are the snapshot's own; each handler reads the options, calls one `App`
//! entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::args::{option_flag, option_list, option_text};
use wg_clispec::registry as view;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{
    Term, human_context, project_create_body, project_option_body, workspace_create_body,
};

use super::Group;
use crate::CliState;

/// The `workspace` group.
pub const WORKSPACE: Group = Group {
    name: "workspace",
    command: view::workspace_command,
    run: workspace,
};

/// The `project` group.
pub const PROJECT: Group = Group {
    name: "project",
    command: view::project_command,
    run: project,
};

fn workspace(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    let reference = option_text(m, "ref").unwrap_or_default();
    match path.get(1).map(String::as_str) {
        Some("create") => {
            let body = workspace_create_body(
                &option_text(m, "slug").unwrap_or_default(),
                option_text(m, "--name").as_deref(),
                &view::option_raw(m, "--prefix").unwrap_or_else(|| "WG".to_owned()),
                &view::option_raw(m, "--description").unwrap_or_default(),
            );
            let out =
                state
                    .app()?
                    .workspace_create(&context, &body, option_flag(m, "--default"))?;
            state.emit(&out, Some(&|c| view::workspace_created_line(c, &out)))
        }
        Some("list") => {
            let app = state.app()?;
            let out = app.workspace_list(&context, option_flag(m, "--all"))?;
            let default = app.default_workspace_slug()?;
            state.emit(
                &out,
                Some(&|c| view::workspace_list_view(c, &out, default.as_deref())),
            )
        }
        Some("show") => {
            let given = option_text(m, "ref");
            let reference = state.workspace_ref(given.as_deref());
            let out = state
                .app()?
                .workspace_summary(&context, reference.as_deref())?;
            state.emit(&out, Some(&|c| view::workspace_show_view(c, &out)))
        }
        Some("use") => {
            let out = state.app()?.workspace_use(&context, &reference)?;
            state.emit(&out, Some(&|c| view::workspace_use_line(c, &out)))
        }
        Some("archive") => {
            let out = state.app()?.workspace_archive(&context, &reference)?;
            state.emit(&out, Some(&|c| view::workspace_archived_line(c, &out)))
        }
        Some("set") => {
            let roots = option_list(m, "--allowed-root");
            if roots.is_empty() {
                return Err(CliFailure::fail("nothing to update", 2));
            }
            let out = state
                .app()?
                .workspace_set_roots(&context, &reference, &roots)?;
            state.emit(&out, Some(&|c| view::workspace_settings_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

fn project(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    let reference = option_text(m, "ref").unwrap_or_default();
    match path.get(1).map(String::as_str) {
        Some("create") => {
            let body = project_create_body(
                &option_text(m, "slug").unwrap_or_default(),
                option_text(m, "--name").as_deref(),
                &view::option_raw(m, "--description").unwrap_or_default(),
                &option_list(m, "--repo"),
            );
            let out = state.app()?.project_create(&context, ws, &body)?;
            state.emit(&out, Some(&|c| view::project_created_line(c, &out)))
        }
        Some("list") => {
            let out = state
                .app()?
                .project_list(&context, ws, option_flag(m, "--all"))?;
            state.emit(&out, Some(&|c| view::project_list_view(c, &out)))
        }
        Some("show") => {
            let out = state.app()?.project_summary(&context, ws, &reference)?;
            state.emit(&out, Some(&|c| view::project_show_view(c, &out)))
        }
        Some("update") => {
            let body = project_option_body(
                view::option_raw(m, "--name").as_deref(),
                view::option_raw(m, "--description").as_deref(),
                view::option_raw(m, "--status").as_deref(),
            );
            let out = state
                .app()?
                .project_update(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::project_updated_line(c, &out)))
        }
        Some("rename-term") => rename_term(state, &context, ws, &reference, m),
        Some("add-repo") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let role = view::option_raw(m, "--role").unwrap_or_else(|| "member".to_owned());
            let out = state.app()?.project_add_repository(
                &context,
                ws,
                &reference,
                &repository,
                &role,
            )?;
            state.emit(&out, Some(&|c| view::project_spans_line(c, &out)))
        }
        Some("remove-repo") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let out =
                state
                    .app()?
                    .project_remove_repository(&context, ws, &reference, &repository)?;
            state.emit(&out, Some(&|c| view::project_spans_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `project rename-term REF --from OLD --to NEW [--keep TOKEN]... [--dry-run]`: exits 1 when an
/// update failed.
fn rename_term(
    state: &mut CliState,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    reference: &str,
    m: &ArgMatches,
) -> Result<(), CliFailure> {
    let keep = option_list(m, "--keep");
    let kept: Vec<&str> = keep.iter().map(String::as_str).collect();
    let term = Term::new(
        &option_text(m, "--from").unwrap_or_default(),
        &option_text(m, "--to").unwrap_or_default(),
        &kept,
    )?;
    let dry_run = option_flag(m, "--dry-run");
    let out = state
        .app()?
        .project_rename_term(context, ws, reference, &term, dry_run)?;
    state.emit(&out, Some(&|c| view::rename_view(c, &out)))?;
    if view::rename_failed(&out) {
        return Err(CliFailure::Exit(1));
    }
    Ok(())
}
