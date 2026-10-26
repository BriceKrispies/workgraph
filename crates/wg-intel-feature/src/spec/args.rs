//! The arguments of the `classify` and `search` commands (`cli/commands/intelligence.py`): the clap
//! commands, which are the snapshot's own trees so usage, options and help match `contracts/cli.json`,
//! and the readers of the matched options. Functions of the matches with no I/O.

use clap::ArgMatches;
use wg_clispec::args::{flag, many, number, one, option_text};
use wg_clispec::tree::SNAPSHOT_JSON;
use wg_clispec::{CliFailure, Snapshot, Usage};

/// The placeholder failure of a command this build does not serve yet.
#[must_use]
pub fn not_served(path: &[String]) -> CliFailure {
    CliFailure::fail(wg_clispec::not_implemented_message(path), 1)
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
        Ok(snapshot) => wg_clispec::tree::placeholder(&snapshot, name),
        Err(_) => wg_clispec::tree::command(name),
    }
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
