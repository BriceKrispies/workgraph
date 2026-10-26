//! `workgraph git sync, log, show, link, unlink, changes, diff, surface, analyze, worktrees,
//! cleanup-worktrees, install-hook` (IF-12).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads the options, calls one `App` entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::git as view;
use wg_clispec::task::{option_flag, option_text};
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::human_context;

use super::Group;
use crate::CliState;

/// The `git` group.
pub const GROUP: Group = Group {
    name: "git",
    command: view::command,
    run,
};

/// An integer option, with its default when it is not given.
fn int_option(m: &ArgMatches, id: &str, default: i64) -> Result<i64, CliFailure> {
    match option_text(m, id) {
        None => Ok(default),
        Some(text) => text.parse().map_err(|_| {
            CliFailure::fail(
                format!("Invalid value for '{id}': '{text}' is not a valid integer."),
                2,
            )
        }),
    }
}

/// A float option, with its default when it is not given.
fn float_option(m: &ArgMatches, id: &str, default: f64) -> Result<f64, CliFailure> {
    match option_text(m, id) {
        None => Ok(default),
        Some(text) => text.parse().map_err(|_| {
            CliFailure::fail(
                format!("Invalid value for '{id}': '{text}' is not a valid float."),
                2,
            )
        }),
    }
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("sync") => {
            let repository = option_text(m, "repository");
            let full = option_flag(m, "--full");
            let out = state
                .app()?
                .git_sync(&context, ws, repository.as_deref(), full)?;
            state.emit(&out, Some(&|c| view::sync_view(c, &out)))
        }
        Some("log") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let limit = int_option(m, "--limit", 30)?;
            let linked = option_flag(m, "--linked");
            let out = state
                .app()?
                .git_log(&context, ws, &repository, limit, linked)?;
            state.emit(&out, Some(&|c| view::log_view(c, &out)))
        }
        Some("show") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let out = state.app()?.git_show(&context, ws, &repository, &sha)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some("link") => {
            let task = option_text(m, "task").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let repository = option_text(m, "--repository");
            let note = option_text(m, "--note").unwrap_or_default();
            let out =
                state
                    .app()?
                    .git_link(&context, ws, &task, &sha, repository.as_deref(), &note)?;
            state.emit(&out, Some(&|c| view::link_line(c, &out)))
        }
        Some("unlink") => {
            let task = option_text(m, "task").unwrap_or_default();
            let sha = option_text(m, "sha").unwrap_or_default();
            let out = state.app()?.git_unlink(&context, ws, &task, &sha)?;
            state.emit(&out, Some(&|c| view::unlink_line(c, &out)))
        }
        Some("changes") => {
            let task = option_text(m, "task").unwrap_or_default();
            let min_confidence = float_option(m, "--min-confidence", 0.5)?;
            let out = state
                .app()?
                .git_changes(&context, ws, &task, min_confidence)?;
            state.emit(&out, Some(&|c| view::changes_view(c, &out)))
        }
        Some("diff") => {
            let task = option_text(m, "task").unwrap_or_default();
            let repository = option_text(m, "--repository");
            let out = state
                .app()?
                .git_diff(&context, ws, &task, repository.as_deref())?;
            state.emit(&out, Some(&|c| view::diff_view(c, &out)))
        }
        Some("surface") => {
            let task = option_text(m, "task").unwrap_or_default();
            let out = state.app()?.git_surface(&context, ws, &task)?;
            state.emit(&out, Some(&|c| view::surface_view(c, &out)))
        }
        Some("analyze") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let limit = int_option(m, "--limit", 200)?;
            let out = state.app()?.git_analyze(&context, ws, &repository, limit)?;
            state.emit(&out, Some(&|c| view::analyze_line(c, &out)))
        }
        Some("worktrees") => {
            let status = option_text(m, "--status");
            let out = state
                .app()?
                .git_worktrees(&context, ws, status.as_deref())?;
            state.emit(&out, Some(&|c| view::worktrees_view(c, &out)))
        }
        Some("cleanup-worktrees") => {
            let apply = option_flag(m, "--apply");
            let out = state.app()?.git_cleanup_worktrees(&context, apply)?;
            state.emit(&out, Some(&|c| view::cleanup_view(c, &out)))
        }
        Some("install-hook") => {
            let repository = option_text(m, "repository").unwrap_or_default();
            let out = state.app()?.git_install_hook(&context, ws, &repository)?;
            state.emit(&out, Some(&|c| view::hook_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
