//! `workgraph classify show, gold, evaluations, disagreements` and `workgraph search` (C038-C046,
//! C133): the reads of the classification history and the lexical search.
//!
//! The clap commands are the snapshot's own trees, so help and parameters match `contracts/cli.json`.
//! The writes of `classify` (`request`, `confirm`, `promote`, `reclassify`, `evaluate`, and `gold`
//! with `--category`) and the `search` options `--reindex` and `--similar-to` need the LLM providers
//! and the job queue, which are not ported; they stay placeholders and fail as such.

use clap::ArgMatches;
use wg_clispec::CliFailure;
use wg_clispec::args::option_text;
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::entry;
use crate::spec::args::{self, not_served};
use crate::spec::search_out::SearchFilters;
use crate::spec::view;

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
        Some("gold") if option_text(m, "--category").is_none() => {
            let out = entry::classify_gold(state.app()?, &context, ws)?;
            state.emit(&out, Some(&|c| view::gold_view(c, &out)))
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

fn run_search(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let search = args::search_args(m)?;
    if search.reindex || search.similar_to.is_some() {
        return Err(not_served(path));
    }
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let context = human_context(None)?;
    let filters = SearchFilters {
        project: search.project,
        repository: search.repository,
        category: search.category,
        statuses: search.statuses,
        agent: search.agent,
        model: search.model,
        changed_path: search.changed_path,
    };
    let out = entry::search(
        state.app()?,
        &context,
        workspace.as_deref(),
        &search.query,
        &search.mode,
        search.limit,
        &filters,
    )?;
    state.emit(&out, Some(&|c| view::search_view(c, &out)))
}
