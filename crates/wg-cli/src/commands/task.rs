//! `workgraph task create, list, show, update, bulk-update, archive, history, graph, ready,
//! promote-ready` (IF-05) and `claim, release, complete, progress, result, comment, depend,
//! relate, link-repo`. The other `task` commands are placeholders until their groups land.
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler builds its request with `wg_clispec::task`, calls one
//! `App` entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::task as view;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{GraphRequest, human_context};

use super::Group;
use crate::{CliState, console};

/// The `task` group.
pub const GROUP: Group = Group {
    name: "task",
    command: view::command,
    run,
};

fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(view::option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("create") => {
            let body = view::create_body(m, &read_file)?;
            let out = state.app()?.task_create(&context, ws, &body)?;
            state.emit(&out, Some(&|c| view::created_line(c, &out)))
        }
        Some("list") => {
            let body = view::list_body(m)?;
            let out = state.app()?.task_list(&context, ws, &body)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("ready") => {
            let limit = view::ready_limit(m)?;
            let project = view::option_text(m, "--project");
            let repo = view::option_text(m, "--repo");
            let agent = view::option_text(m, "--agent");
            let out = state.app()?.task_ready(
                &context,
                ws,
                project.as_deref(),
                repo.as_deref(),
                agent.as_deref(),
                limit,
            )?;
            state.emit(&out, Some(&|c| view::ready_view(c, &out)))
        }
        Some("show") => show(state, &context, ws, m),
        Some("update") => {
            let body = view::update_body(m, &read_file)?;
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state.app()?.task_update(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::updated_line(c, &out)))
        }
        Some("bulk-update") => {
            let out = state
                .app()?
                .task_bulk_update(&context, ws, &view::bulk_body(m))?;
            state.emit(&out, Some(&|c| view::bulk_view(c, &out)))?;
            if view::bulk_failed(&out) {
                return Err(CliFailure::Exit(3));
            }
            Ok(())
        }
        Some("archive") => {
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let undo = view::option_flag(m, "--undo");
            let out = state.app()?.task_archive(&context, ws, &reference, !undo)?;
            state.emit(&out, Some(&|c| view::archived_line(c, &out, undo)))
        }
        Some("history") => {
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let types = view::option_list(m, "--type");
            let out = state
                .app()?
                .task_history(&context, ws, &reference, &types)?;
            state.emit(&out, Some(&|c| view::history_view(c, &out)))
        }
        Some("graph") => {
            let root = view::option_text(m, "--root");
            let project = view::option_text(m, "--project");
            let repository = view::option_text(m, "--repo");
            let request = GraphRequest {
                root: root.as_deref(),
                project: project.as_deref(),
                repository: repository.as_deref(),
                include_closed: !view::option_flag(m, "--open"),
            };
            let out = state.app()?.task_graph(&context, ws, &request)?;
            state.emit(&out, Some(&|c| view::graph_view(c, &out)))
        }
        Some("promote-ready") => {
            let (tags, project, dry_run) = view::promote_scope(m)?;
            let out = state.app()?.task_promote_ready(
                &context,
                ws,
                &tags,
                project.as_deref(),
                dry_run,
            )?;
            state.emit(&out, Some(&|c| view::promote_view(c, &out, dry_run)))
        }
        Some("claim") => {
            let body = view::claim_body(m)?;
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state.app()?.task_claim(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::claimed_line(c, &out)))
        }
        Some("release") => {
            let body = view::release_body(m);
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state.app()?.task_release(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::released_line(c, &out)))
        }
        Some("complete") => {
            let body = view::complete_body(m);
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state
                .app()?
                .task_complete(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::status_line(c, &out)))
        }
        Some("result") => {
            let body = view::result_body(m)?;
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state.app()?.task_result(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::status_line(c, &out)))
        }
        Some("progress") => {
            let body = view::progress_body(m)?;
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state
                .app()?
                .task_progress(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::note_line(c, "progress recorded")))
        }
        Some("comment") => {
            let body = view::comment_body(m);
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state.app()?.task_comment(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::note_line(c, "comment added")))
        }
        Some("depend") => {
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let depends_on = view::option_text(m, "depends_on").unwrap_or_default();
            let app = state.app()?;
            let out = if view::option_flag(m, "--remove") {
                app.task_remove_dependency(&context, ws, &reference, &depends_on)?
            } else {
                let body = view::dependency_body(&depends_on);
                app.task_add_dependency(&context, ws, &reference, &body)?
            };
            state.emit(&out, Some(&|c| view::depend_line(c, &out)))
        }
        Some("relate") => {
            let body = view::relation_body(m);
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let out = state
                .app()?
                .task_add_relation(&context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| view::related_line(c, &out)))
        }
        Some("link-repo") => {
            let reference = view::option_text(m, "ref").unwrap_or_default();
            let app = state.app()?;
            let out = if view::option_flag(m, "--remove") {
                let repository = view::option_text(m, "repository").unwrap_or_default();
                app.task_unlink_repository(&context, ws, &reference, &repository)?
            } else {
                let body = view::repo_body(m);
                app.task_link_repository(&context, ws, &reference, &body)?
            };
            state.emit(&out, Some(&|c| view::linked_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `task show REF...`: one reference prints the task; several print `{items, errors}` and exit
/// with the first failure's code.
fn show(
    state: &mut CliState,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    m: &ArgMatches,
) -> Result<(), CliFailure> {
    let refs = view::option_list(m, "refs");
    let shown = state.app()?.task_show(context, ws, &refs)?;
    if refs.len() == 1 {
        if let Some((_, error)) = shown.errors.into_iter().next() {
            return Err(CliFailure::Domain(error));
        }
        let Some(task) = shown.items.first() else {
            return Ok(());
        };
        return state.emit(task, Some(&|c| view::task_view(c, task)));
    }
    let code = shown.errors.first().map(|(_, error)| error.exit_code());
    if state.json {
        let value = view::shown_value(shown.items, &shown.errors);
        state.emit(&value, None)?;
    } else {
        let (stdout, stderr) =
            view::shown_view(&state.console, &console(true), &shown.items, &shown.errors);
        state.print(&stdout);
        state.eprint(&stderr);
    }
    match code {
        Some(code) => Err(CliFailure::Exit(code)),
        None => Ok(()),
    }
}
