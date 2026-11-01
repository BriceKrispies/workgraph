//! `workgraph task create, list, show, update, bulk-update, archive, history, graph, ready,
//! promote-ready` (IF-05), `import-breakdown` and `claim, release, complete, accept, reject, progress, result, comment, depend,
//! relate, link-repo`. The other `task` commands are placeholders until their groups land.
//!
//! The clap command is the snapshot's own tree, so help and parameters match
//! `contracts/cli.json`; each handler builds its request with `crate::spec::args`, calls one
//! `App` entry point and prints the result.

use crate::service;
use crate::spec::args::*;
use crate::spec::breakdown::import_view;
use crate::spec::contract::contract_target;
use crate::spec::reads::GraphRequest;
use crate::spec::triage::{triage_failed, triage_view};
use crate::spec::view::*;
use clap::ArgMatches;
use wg_iface::cli::args::*;
use wg_iface::cli::{CliFailure, PathKind, PathProblem, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

/// The `task` group.
pub const GROUP: CliGroup = CliGroup {
    name: "task",
    command,
    run,
};

fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("create") => {
            let body = create_body(m, &read_file)?;
            let out = service::task_create(state.app()?, &context, ws, &body)?;
            state.emit(&out, Some(&|c| created_line(c, &out)))
        }
        Some("list") => {
            let body = list_body(m)?;
            let out = service::task_list(state.app()?, &context, ws, &body)?;
            state.emit(&out, Some(&|c| list_view(c, &out)))
        }
        Some("ready") => {
            let limit = ready_limit(m)?;
            let project = option_text(m, "--project");
            let repo = option_text(m, "--repo");
            let agent = option_text(m, "--agent");
            let out = service::task_ready(
                state.app()?,
                &context,
                ws,
                project.as_deref(),
                repo.as_deref(),
                agent.as_deref(),
                limit,
            )?;
            state.emit(&out, Some(&|c| ready_view(c, &out)))
        }
        Some("show") => show(state, &context, ws, m),
        Some("update") => {
            let body = update_body(m, &read_file)?;
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_update(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| updated_line(c, &out)))
        }
        Some("bulk-update") => {
            let out = service::task_bulk_update(state.app()?, &context, ws, &bulk_body(m))?;
            state.emit(&out, Some(&|c| bulk_view(c, &out)))?;
            if bulk_failed(&out) {
                return Err(CliFailure::Exit(3));
            }
            Ok(())
        }
        Some("archive") => {
            let reference = option_text(m, "ref").unwrap_or_default();
            let undo = option_flag(m, "--undo");
            let out = service::task_archive(state.app()?, &context, ws, &reference, !undo)?;
            state.emit(&out, Some(&|c| archived_line(c, &out, undo)))
        }
        Some("history") => {
            let reference = option_text(m, "ref").unwrap_or_default();
            let types = option_list(m, "--type");
            let out = service::task_history(state.app()?, &context, ws, &reference, &types)?;
            state.emit(&out, Some(&|c| history_view(c, &out)))
        }
        Some("graph") => {
            let root = option_text(m, "--root");
            let project = option_text(m, "--project");
            let repository = option_text(m, "--repo");
            let request = GraphRequest {
                root: root.as_deref(),
                project: project.as_deref(),
                repository: repository.as_deref(),
                include_closed: !option_flag(m, "--open"),
            };
            let out = service::task_graph(state.app()?, &context, ws, &request)?;
            state.emit(&out, Some(&|c| graph_view(c, &out)))
        }
        Some("promote-ready") => {
            let (tags, project, dry_run) = promote_scope(m)?;
            let out = service::task_promote_ready(
                state.app()?,
                &context,
                ws,
                &tags,
                project.as_deref(),
                dry_run,
            )?;
            state.emit(&out, Some(&|c| promote_view(c, &out, dry_run)))
        }
        Some("claim") => {
            let body = claim_body(m)?;
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_claim(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| claimed_line(c, &out)))
        }
        Some("release") => {
            let body = release_body(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_release(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| released_line(c, &out)))
        }
        Some("complete") => {
            let body = complete_body(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_complete(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| status_line(c, &out)))
        }
        Some("accept") => {
            let (manual, reason, promote) = accept_flags(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let app = state.app()?;
            let out = if promote {
                service::acceptance::task_accept_promoting(
                    app, &context, ws, &reference, manual, &reason,
                )?
            } else if manual {
                service::acceptance::task_accept_manually(app, &context, ws, &reference, &reason)?
            } else {
                service::acceptance::task_accept(app, &context, ws, &reference, &reason)?
            };
            state.emit(&out, Some(&|c| accepted_line(c, &out, promote)))
        }
        Some("reject") => {
            let reason = reject_reason(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let out =
                service::acceptance::task_reject(state.app()?, &context, ws, &reference, &reason)?;
            state.emit(&out, Some(&|c| rejected_line(c)))
        }
        Some("result") => {
            let body = result_body(m)?;
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_result(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| status_line(c, &out)))
        }
        Some("progress") => {
            let body = progress_body(m)?;
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_progress(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| note_line(c, "progress recorded")))
        }
        Some("comment") => {
            let body = comment_body(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_comment(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| note_line(c, "comment added")))
        }
        Some("contract-check") => {
            let reference = option_text(m, "ref").unwrap_or_default();
            let model = one(m, "--model").unwrap_or_default();
            let target = contract_target(one(m, "--target"))?;
            let out = service::contract::task_contract_check(
                state.app()?,
                &context,
                ws,
                &reference,
                &model,
                target,
            )?;
            state.emit(&out, Some(&|c| contract_view(c, &out)))
        }
        Some("triage-apply") => {
            let path = option_text(m, "path").unwrap_or_default();
            check_path(&TRIAGE_APPLY, &path)?;
            let dry_run = option_flag(m, "--dry-run");
            let source = option_text(m, "--source");
            let out = service::triage::task_triage_apply(
                state.app()?,
                &context,
                ws,
                &path,
                dry_run,
                source.as_deref(),
            )?;
            state.emit(&out, Some(&|c| triage_view(c, &out, dry_run)))?;
            if triage_failed(&out) {
                return Err(CliFailure::Exit(1));
            }
            Ok(())
        }
        Some("import-breakdown") => {
            let path = option_text(m, "path").unwrap_or_default();
            check_path(&IMPORT_BREAKDOWN, &path)?;
            let markdown = read_file(&path).map_err(|message| CliFailure::fail(message, 1))?;
            let tags = tag_values(&option_list(m, "--tag"));
            let section = option_text(m, "--section").unwrap_or_default();
            let parent = option_text(m, "--parent").unwrap_or_default();
            let project = option_text(m, "--project");
            let repository = option_text(m, "--repo");
            let source = source_name(&path);
            let request = service::breakdown::ImportRequest {
                markdown: &markdown,
                section: &section,
                source: &source,
                parent: &parent,
                tags: &tags,
                project: project.as_deref(),
                repository: repository.as_deref(),
                dry_run: option_flag(m, "--dry-run"),
            };
            let out =
                service::breakdown::task_import_breakdown(state.app()?, &context, ws, &request)?;
            state.emit(&out, Some(&|c| import_view(c, &out)))
        }
        Some("depend") => {
            let reference = option_text(m, "ref").unwrap_or_default();
            let depends_on = option_text(m, "depends_on").unwrap_or_default();
            let app = state.app()?;
            let out = if option_flag(m, "--remove") {
                service::task_remove_dependency(app, &context, ws, &reference, &depends_on)?
            } else {
                let body = dependency_body(&depends_on);
                service::task_add_dependency(app, &context, ws, &reference, &body)?
            };
            state.emit(&out, Some(&|c| depend_line(c, &out)))
        }
        Some("relate") => {
            let body = relation_body(m);
            let reference = option_text(m, "ref").unwrap_or_default();
            let out = service::task_add_relation(state.app()?, &context, ws, &reference, &body)?;
            state.emit(&out, Some(&|c| related_line(c, &out)))
        }
        Some("link-repo") => {
            let reference = option_text(m, "ref").unwrap_or_default();
            let app = state.app()?;
            let out = if option_flag(m, "--remove") {
                let repository = option_text(m, "repository").unwrap_or_default();
                service::task_unlink_repository(app, &context, ws, &reference, &repository)?
            } else {
                let body = repo_body(m);
                service::task_link_repository(app, &context, ws, &reference, &body)?
            };
            state.emit(&out, Some(&|c| linked_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// The usage of `task triage-apply`, as Click prints it for its `PATH` argument.
const TRIAGE_APPLY: wg_iface::cli::Usage = wg_iface::cli::Usage {
    usage: "workgraph task triage-apply [OPTIONS] PATH",
    command: "workgraph task triage-apply",
};

/// The usage of `task import-breakdown`, as Click prints it for its `PATH` argument.
const IMPORT_BREAKDOWN: wg_iface::cli::Usage = wg_iface::cli::Usage {
    usage: "workgraph task import-breakdown [OPTIONS] PATH",
    command: "workgraph task import-breakdown",
};

/// `path.resolve().as_posix()`: the absolute path of the document, with forward slashes. It names
/// the document in the metadata of every task the import creates.
fn source_name(path: &str) -> String {
    let resolved =
        std::fs::canonicalize(path).map_or_else(|_| path.to_owned(), |p| p.display().to_string());
    resolved
        .strip_prefix(r"\\?\")
        .unwrap_or(&resolved)
        .replace('\\', "/")
}

/// Click's `Path(exists=True, dir_okay=False, readable=True)` check of the `PATH` argument, which
/// runs before the command: a usage error (exit 2) for a missing file, a directory or an
/// unreadable one.
fn check_path(usage: &wg_iface::cli::Usage, path: &str) -> Result<(), CliFailure> {
    let problem = match std::fs::metadata(path) {
        Err(_) => PathProblem::Missing,
        Ok(meta) if meta.is_dir() => PathProblem::IsDirectory,
        Ok(_) if std::fs::File::open(path).is_err() => PathProblem::Unreadable,
        Ok(_) => return Ok(()),
    };
    Err(usage.invalid_path("PATH", path, PathKind::File, problem))
}

/// `task show REF...`: one reference prints the task; several print `{items, errors}` and exit
/// with the first failure's code.
fn show(
    state: &mut CliState,
    context: &wg_runtime::RequestContext,
    ws: Option<&str>,
    m: &ArgMatches,
) -> Result<(), CliFailure> {
    let refs = option_list(m, "refs");
    let shown = service::task_show(state.app()?, context, ws, &refs)?;
    if refs.len() == 1 {
        if let Some((_, error)) = shown.errors.into_iter().next() {
            return Err(CliFailure::Domain(error));
        }
        let Some(task) = shown.items.first() else {
            return Ok(());
        };
        return state.emit(task, Some(&|c| task_view(c, task)));
    }
    let code = shown.errors.first().map(|(_, error)| error.exit_code());
    if state.json {
        let value = shown_value(shown.items, &shown.errors);
        state.emit(&value, None)?;
    } else {
        let (stdout, stderr) = shown_view(
            &state.console,
            &state.err_console,
            &shown.items,
            &shown.errors,
        );
        state.print(&stdout);
        state.eprint(&stderr);
    }
    match code {
        Some(code) => Err(CliFailure::Exit(code)),
        None => Ok(()),
    }
}
