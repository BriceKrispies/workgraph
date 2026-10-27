//! `workgraph git sync, log, show, link, unlink, changes, diff, surface, analyze, worktrees,
//! cleanup-worktrees, install-hook` (IF-12).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads the options, calls one [`crate::service`] entry point and prints the result
//! with [`crate::spec::cli`]'s views.

use clap::ArgMatches;
use wg_clispec::args::{option_flag, option_text};
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service;
use crate::spec::cli as view;

/// The `git` group.
pub const GROUP: CliGroup = CliGroup {
    name: "git",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("sync") => {
            let repository = option_text(m, "repository");
            let full = option_flag(m, "--full");
            let out = service::git_sync(state.app()?, &context, ws, repository.as_deref(), full)?;
            state.emit(&out, Some(&|c| view::sync_view(c, &out)))
        }
        Some("log") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let limit = view::int_option(m, "--limit", 30)?;
            let linked = option_flag(m, "--linked");
            let out = service::git_log(
                state.app()?,
                &context,
                ws,
                &repository,
                limit,
                linked,
                0,
                false,
            )?;
            state.emit(&out, Some(&|c| view::log_view(c, &out)))
        }
        Some("show") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let out = service::git_show(state.app()?, &context, ws, &repository, &sha)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some("link") => {
            let task = option_text(m, "task").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let repository = option_text(m, "--repository");
            let note = option_text(m, "--note").unwrap_or_default();
            let out = service::git_link(
                state.app()?,
                &context,
                ws,
                &task,
                &sha,
                repository.as_deref(),
                &note,
            )?;
            state.emit(&out, Some(&|c| view::link_line(c, &out)))
        }
        Some("unlink") => {
            let task = option_text(m, "task").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let out = service::git_unlink(state.app()?, &context, ws, &task, &sha)?;
            state.emit(&out, Some(&|c| view::unlink_line(c, &out)))
        }
        Some("changes") => {
            let task = option_text(m, "task").unwrap_or_default();
            let min_confidence = view::float_option(m, "--min-confidence", 0.5)?;
            let out = service::git_changes(state.app()?, &context, ws, &task, min_confidence)?;
            state.emit(&out, Some(&|c| view::changes_view(c, &out)))
        }
        Some("diff") => {
            let task = option_text(m, "task").unwrap_or_default();
            let repository = option_text(m, "--repository");
            let out = service::git_diff(state.app()?, &context, ws, &task, repository.as_deref())?;
            state.emit(&out, Some(&|c| view::diff_view(c, &out)))
        }
        Some("surface") => {
            let task = option_text(m, "task").unwrap_or_default();
            let out = service::git_surface(state.app()?, &context, ws, &task)?;
            state.emit(&out, Some(&|c| view::surface_view(c, &out)))
        }
        Some("analyze") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let limit = view::int_option(m, "--limit", 200)?;
            let out = service::git_analyze(state.app()?, &context, ws, &repository, limit)?;
            state.emit(&out, Some(&|c| view::analyze_line(c, &out)))
        }
        Some("worktrees") => {
            let status = option_text(m, "--status");
            let out = service::git_worktrees(state.app()?, &context, ws, status.as_deref())?;
            state.emit(&out, Some(&|c| view::worktrees_view(c, &out)))
        }
        Some("cleanup-worktrees") => {
            let apply = option_flag(m, "--apply");
            let out = service::git_cleanup_worktrees(state.app()?, &context, apply)?;
            state.emit(&out, Some(&|c| view::cleanup_view(c, &out)))
        }
        Some("install-hook") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let out = service::git_install_hook(state.app()?, &context, ws, &repository)?;
            state.emit(&out, Some(&|c| view::hook_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
