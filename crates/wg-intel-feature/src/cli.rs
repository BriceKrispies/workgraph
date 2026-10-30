//! `workgraph classify ...` and `workgraph search` (C038-C046, C133): the classification reads and
//! writes, and the search with its semantic, similar-task and reindex forms.
//!
//! The clap commands are the snapshot's own trees, so help and parameters match `contracts/cli.json`.
//! The writes run the providers of the configuration (`service::llm::Live`) and the job queue.

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{flag, option_text};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::entry;
use crate::service::llm::Live;
use crate::spec::args::{self, not_served};
use crate::spec::requests::{ConfirmRequest, ReclassifyRequest, SearchRequest};
use crate::spec::search_out::SearchFilters;
use crate::spec::view;
use crate::spec::write_view;

/// The `classify` group.
pub const CLASSIFY: CliGroup = CliGroup {
    name: "classify",
    command: args::classify_command,
    run: run_classify,
};

/// The `search` command.
pub const SEARCH: CliGroup = CliGroup {
    name: "search",
    command: args::search_command,
    run: run_search,
};

fn run_classify(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("show") => {
            let reference = args::task_ref(m);
            let out = entry::classify_show(state.app()?, &context, ws, &reference)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some("request") => {
            let task = args::task_ref(m);
            let options = args::run_options(m);
            if flag(m, "--now") {
                let out =
                    entry::classify_now(state.app()?, &context, &mut Live, ws, &task, &options)?;
                state.emit(&out, Some(&|_| write_view::now_line(&out)))
            } else {
                let out = entry::classify_request(state.app()?, &context, ws, &task, &options)?;
                state.emit(&out, Some(&|_| write_view::queued_line(&out)))
            }
        }
        Some("confirm") => {
            let request = ConfirmRequest {
                category: args::category_arg(m),
                subtype: None,
                domain: None,
                complexity: option_text(m, "--complexity"),
                tags: Vec::new(),
                note: option_text(m, "--note").unwrap_or_default(),
                add_to_gold: true,
            };
            let out =
                entry::classify_confirm(state.app()?, &context, ws, &args::task_ref(m), &request)?;
            state.emit(&out, Some(&|_| write_view::confirmed_line(&out)))
        }
        Some("promote") => {
            let out =
                entry::classify_promote(state.app()?, &context, &args::classification_arg(m))?;
            state.emit(&out, Some(&|_| write_view::promoted_line(&out)))
        }
        Some("reclassify") => {
            let request = ReclassifyRequest {
                provider: option_text(m, "--provider"),
                model: option_text(m, "--model"),
                promote: flag(m, "--promote"),
                category: option_text(m, "--category"),
            };
            let out = entry::classify_reclassify(state.app()?, &context, ws, &request)?;
            state.emit(&out, Some(&|_| write_view::reclassified_line(&out)))
        }
        Some("gold") if option_text(m, "--category").is_some() => {
            let category = option_text(m, "--category").unwrap_or_default();
            let out = entry::classify_gold_add(
                state.app()?,
                &context,
                ws,
                &category,
                option_text(m, "--task").as_deref(),
                option_text(m, "--title").as_deref(),
            )?;
            state.emit(&out, Some(&|_| write_view::gold_added_line(&out)))
        }
        Some("gold") => {
            let out = entry::classify_gold(state.app()?, &context, ws)?;
            state.emit(&out, Some(&|c| view::gold_view(c, &out)))
        }
        Some("evaluate") => {
            let out = entry::classify_evaluate(
                state.app()?,
                &context,
                &mut Live,
                ws,
                option_text(m, "--provider").as_deref(),
                option_text(m, "--model").as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::evaluation_view(c, &out)))
        }
        Some("evaluations") => {
            let out = entry::classify_evaluations(state.app()?, &context, ws)?;
            state.emit(&out, Some(&|c| view::evaluations_view(c, &out)))
        }
        Some("disagreements") => {
            let out = entry::classify_disagreements(state.app()?, &context, ws)?;
            state.emit(&out, Some(&|c| view::disagreements_view(c, &out)))
        }
        _ => Err(not_served(path)),
    }
}

fn run_search(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let search = args::search_args(m)?;
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let context = human_context(None)?;
    if search.reindex {
        let out = entry::search_reindex(state.app()?, &context, workspace.as_deref(), None, None)?;
        return state.emit(&out, Some(&|_| write_view::reindex_line(&out)));
    }
    if let Some(reference) = search.similar_to.as_deref() {
        let limit = usize::try_from(search.limit.max(0)).unwrap_or(usize::MAX);
        let out = entry::search_similar(
            state.app()?,
            &context,
            workspace.as_deref(),
            reference,
            limit,
        )?;
        return state.emit(&out, Some(&|c| view::similar_view(c, &out)));
    }
    let request = SearchRequest {
        query: search.query,
        mode: search.mode,
        limit: search.limit,
        filters: SearchFilters {
            project: search.project,
            repository: search.repository,
            category: search.category,
            statuses: search.statuses,
            agent: search.agent,
            model: search.model,
            changed_path: search.changed_path,
            created_after: None,
            created_before: None,
        },
    };
    let out = entry::search(
        state.app()?,
        &context,
        &mut Live,
        workspace.as_deref(),
        &request,
    )?;
    state.emit(&out, Some(&|c| view::search_view(c, &out)))
}
