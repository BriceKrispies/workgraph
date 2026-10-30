//! The arguments of the `classify` and `search` commands (`cli/commands/intelligence.py`): the clap
//! commands, which are the snapshot's own trees so usage, options and help match `contracts/cli.json`,
//! and the readers of the matched options. Functions of the matches with no I/O.

use crate::spec::requests::RunOptions;
use clap::ArgMatches;
use wg_iface::cli::args::{flag, many, number, one, option_text};
use wg_iface::cli::tree::SNAPSHOT_JSON;
use wg_iface::cli::{CliFailure, Snapshot, Usage};

/// The placeholder failure of a command this build does not serve yet.
#[must_use]
pub fn not_served(path: &[String]) -> CliFailure {
    CliFailure::fail(wg_iface::cli::not_implemented_message(path), 1)
}

/// Click's usage line of `search`.
const SEARCH: Usage = Usage {
    usage: "workgraph search [OPTIONS] QUERY",
    command: "workgraph search",
};

/// The clap command of the `classify` group: the snapshot's own tree.
#[must_use]
pub fn classify_command() -> clap::Command {
    group_command("classify")
}

/// The clap command of `search`: the snapshot's own leaf.
#[must_use]
pub fn search_command() -> clap::Command {
    group_command("search")
}

fn group_command(name: &str) -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => wg_iface::cli::tree::placeholder(&snapshot, name),
        Err(_) => wg_iface::cli::tree::command(name),
    }
}

/// The options a classification run takes: `--provider`, `--model` and `--promote`.
#[must_use]
pub fn run_options(m: &ArgMatches) -> RunOptions {
    RunOptions {
        provider: option_text(m, "--provider"),
        model: option_text(m, "--model"),
        promote: flag(m, "--promote"),
        batch_id: None,
    }
}

/// The positional `category` of `classify confirm`.
#[must_use]
pub fn category_arg(m: &ArgMatches) -> String {
    one(m, "category").unwrap_or_default()
}

/// The positional `classification_id` of `classify promote`.
#[must_use]
pub fn classification_arg(m: &ArgMatches) -> String {
    one(m, "classification_id").unwrap_or_default()
}

/// The positional `task` of the `classify` commands that take one.
#[must_use]
pub fn task_ref(m: &ArgMatches) -> String {
    one(m, "task").unwrap_or_default()
}

/// The filters and options of `search`, read from the matches.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchArgs {
    pub query: String,
    pub mode: String,
    pub project: Option<String>,
    pub repository: Option<String>,
    pub category: Option<String>,
    pub statuses: Vec<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub changed_path: Option<String>,
    pub limit: i64,
    pub similar_to: Option<String>,
    pub reindex: bool,
}

/// Reads the `search` options. `--mode` defaults to `hybrid` and `--limit` to 20.
pub fn search_args(m: &ArgMatches) -> Result<SearchArgs, CliFailure> {
    Ok(SearchArgs {
        query: one(m, "query").unwrap_or_default(),
        mode: one(m, "--mode").unwrap_or_else(|| "hybrid".to_owned()),
        project: option_text(m, "--project"),
        repository: option_text(m, "--repo"),
        category: option_text(m, "--category"),
        statuses: many(m, "--status"),
        agent: option_text(m, "--agent"),
        model: option_text(m, "--model"),
        changed_path: option_text(m, "--path"),
        limit: number(m, "--limit", 20, SEARCH)?,
        similar_to: option_text(m, "--similar-to"),
        reindex: flag(m, "--reindex"),
    })
}
