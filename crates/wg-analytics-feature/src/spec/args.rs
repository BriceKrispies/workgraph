//! Pure: the arguments of the `analytics` commands (`cli/commands/analytics.py`): the clap command,
//! which is the snapshot's own tree so help and parameters match `contracts/cli.json`, and the
//! readers of the matched options (typer's types, Click's usage errors). The handlers in
//! [`crate::cli`] run them.

use clap::ArgMatches;
use wg_clispec::args::{flag, number, one, option_list, option_text};
use wg_clispec::datetime::parse_click_datetime;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{CliFailure, Snapshot, Usage};
use wg_json::LocalDateTime;

use crate::spec::analyses::Typed;
use crate::spec::query::QueryRequest;

/// Click's usage line of `analytics query`.
const QUERY: Usage = Usage {
    usage: "workgraph analytics query [OPTIONS] {metric}",
    command: "workgraph analytics query",
};

/// Click's usage line of `analytics compare`.
const COMPARE: Usage = Usage {
    usage: "workgraph analytics compare [OPTIONS] {metric} {a_since} {a_until} {b_since} {b_until}",
    command: "workgraph analytics compare",
};

/// Click's usage line of `analytics coupling`.
const COUPLING: Usage = Usage {
    usage: "workgraph analytics coupling [OPTIONS]",
    command: "workgraph analytics coupling",
};

/// Click's usage line of `analytics retrospective`.
const RETROSPECTIVE: Usage = Usage {
    usage: "workgraph analytics retrospective [OPTIONS]",
    command: "workgraph analytics retrospective",
};

/// Click's usage line of `analytics history`.
const HISTORY: Usage = Usage {
    usage: "workgraph analytics history [OPTIONS]",
    command: "workgraph analytics history",
};

/// The `compare` arguments: the metric, both windows and the `--by` dimensions.
pub type CompareArguments = (String, Typed, Typed, Vec<String>);

/// The `history` options: the category, the repository and the limit.
pub type HistoryOptions = (Option<String>, Option<String>, i64);

/// The `retrospective` options: the period in days, the project, the repository, whether the model
/// narrates (`--no-model` turns it off) and the `--output` file.
pub type RetrospectiveOptions = (
    i64,
    Option<String>,
    Option<String>,
    bool,
    Option<std::path::PathBuf>,
);

/// The `retrospective` options as the command reads them (`--days` defaults to 14).
pub fn retrospective_options(m: &ArgMatches) -> Result<RetrospectiveOptions, CliFailure> {
    Ok((
        number(m, "--days", 14, RETROSPECTIVE)?,
        option_text(m, "--project"),
        option_text(m, "--repo"),
        !flag(m, "--no-model"),
        option_text(m, "--output").map(std::path::PathBuf::from),
    ))
}

/// The `estimate` options: the task, the title, the category and the `--repo` repositories.
pub type EstimateOptions = (Option<String>, Option<String>, Option<String>, Vec<String>);

/// The `estimate` arguments and options, as the command reads them.
#[must_use]
pub fn estimate_options(m: &ArgMatches) -> EstimateOptions {
    (
        one(m, "task"),
        option_text(m, "--title"),
        option_text(m, "--category"),
        option_list(m, "--repo"),
    )
}

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
        since: click_datetime(m, "--since", QUERY)?,
        until: click_datetime(m, "--until", QUERY)?,
    })
}

/// `overview --period` (default `week`).
#[must_use]
pub fn overview_period(m: &ArgMatches) -> String {
    one(m, "--period").unwrap_or_else(|| "week".to_owned())
}

/// The `compare` arguments: the metric, the two windows and the `--by` dimensions.
pub fn compare_arguments(m: &ArgMatches) -> Result<CompareArguments, CliFailure> {
    let at = |name: &str| -> Result<LocalDateTime, CliFailure> {
        click_datetime(m, name, COMPARE)?.ok_or_else(|| CliFailure::Usage {
            usage: COMPARE.usage.to_owned(),
            command: COMPARE.command.to_owned(),
            message: format!("Missing argument '{name}'."),
        })
    };
    Ok((
        one(m, "metric").unwrap_or_default(),
        (at("a_since")?, at("a_until")?),
        (at("b_since")?, at("b_until")?),
        option_list(m, "--by"),
    ))
}

/// The `coupling` options: `--project` and `--min-support` (default 1).
pub fn coupling_options(m: &ArgMatches) -> Result<(Option<String>, i64), CliFailure> {
    Ok((
        option_text(m, "--project"),
        number(m, "--min-support", 1, COUPLING)?,
    ))
}

/// `failures --persist`: off unless given; `--no-persist` turns it off again.
#[must_use]
pub fn failures_persist(m: &ArgMatches) -> bool {
    flag(m, "--persist") && !flag(m, "--persist__secondary")
}

/// The `history` options: the category, the repository and the limit (default 30).
pub fn history_options(m: &ArgMatches) -> Result<HistoryOptions, CliFailure> {
    Ok((
        option_text(m, "--category"),
        option_text(m, "--repo"),
        number(m, "--limit", 30, HISTORY)?,
    ))
}

/// A `datetime` option or argument: Click's `BadParameter` when no format reads it.
fn click_datetime(
    m: &ArgMatches,
    name: &str,
    at: Usage,
) -> Result<Option<LocalDateTime>, CliFailure> {
    match one(m, name) {
        None => Ok(None),
        Some(text) => parse_click_datetime(&text)
            .map(Some)
            .map_err(|message| CliFailure::Usage {
                usage: at.usage.to_owned(),
                command: at.command.to_owned(),
                message: format!("Invalid value for '{name}': {message}"),
            }),
    }
}
