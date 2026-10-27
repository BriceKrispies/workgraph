//! Pure: the arguments of the `analytics` commands (`cli/commands/analytics.py`): the clap command,
//! which is the snapshot's own tree so help and parameters match `contracts/cli.json`, and the
//! readers of the matched options (typer's types, Click's usage errors). The handlers in
//! [`crate::cli`] run them.

use clap::ArgMatches;
use wg_clispec::args::{one, option_list, option_text};
use wg_clispec::datetime::parse_click_datetime;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{CliFailure, Snapshot, Usage};
use wg_json::LocalDateTime;

use crate::spec::query::QueryRequest;

/// Click's usage line of `analytics query`.
const QUERY: Usage = Usage {
    usage: "workgraph analytics query [OPTIONS] {metric}",
    command: "workgraph analytics query",
};

/// The clap command of the `analytics` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "analytics"),
        Err(_) => wg_clispec::tree::command("analytics"),
    }
}

/// The `query` options as the command reads them. A `datetime` option is parsed as typer does.
pub fn query_request(m: &ArgMatches) -> Result<QueryRequest, CliFailure> {
    Ok(QueryRequest {
        metric: one(m, "metric").unwrap_or_default(),
        group_by: option_list(m, "--by"),
        period: one(m, "--period").unwrap_or_else(|| "week".to_owned()),
        project: option_text(m, "--project"),
        repository: option_text(m, "--repo"),
        category: option_text(m, "--category"),
        provider: option_text(m, "--provider"),
        model: option_text(m, "--model"),
        since: click_datetime(m, "--since")?,
        until: click_datetime(m, "--until")?,
    })
}

/// A `datetime` option: Click's `BadParameter` when no format reads it.
fn click_datetime(m: &ArgMatches, option: &str) -> Result<Option<LocalDateTime>, CliFailure> {
    match one(m, option) {
        None => Ok(None),
        Some(text) => parse_click_datetime(&text)
            .map(Some)
            .map_err(|message| CliFailure::Usage {
                usage: QUERY.usage.to_owned(),
                command: QUERY.command.to_owned(),
                message: format!("Invalid value for '{option}': {message}"),
            }),
    }
}
