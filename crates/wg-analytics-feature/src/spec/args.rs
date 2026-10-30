//! Pure: the arguments of the `analytics` commands (`cli/commands/analytics.py`): the clap command,
//! which is the snapshot's own tree so help and parameters match `contracts/cli.json`, and the
//! readers of the matched options (typer's types, Click's usage errors). The handlers in
//! [`crate::cli`] run them.

use clap::ArgMatches;
use serde_json::{Map, Value as Json};
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

/// Click's usage line of `analytics subagents`.
const SUBAGENTS: Usage = Usage {
    usage: "workgraph analytics subagents [OPTIONS]",
    command: "workgraph analytics subagents",
};

/// Click's usage line of `analytics time`.
const TIME: Usage = Usage {
    usage: "workgraph analytics time [OPTIONS]",
    command: "workgraph analytics time",
};

/// Click's usage line of `analytics bottlenecks`.
const BOTTLENECKS: Usage = Usage {
    usage: "workgraph analytics bottlenecks [OPTIONS]",
    command: "workgraph analytics bottlenecks",
};

/// The `subagents` options: the period (`week` or `month`), the window and the model.
pub type SubagentsOptions = (
    String,
    Option<LocalDateTime>,
    Option<LocalDateTime>,
    Option<String>,
);

/// The `subagents` options as the command reads them (`--period` defaults to `week`).
pub fn subagents_options(m: &ArgMatches) -> Result<SubagentsOptions, CliFailure> {
    Ok((
        one(m, "--period").unwrap_or_else(|| "week".to_owned()),
        click_datetime(m, "--since", SUBAGENTS)?,
        click_datetime(m, "--until", SUBAGENTS)?,
        option_text(m, "--model"),
    ))
}

/// The `time` options: the window, the grouping (`--by`, default `class`) and the model.
pub type TimeOptions = (
    Option<LocalDateTime>,
    Option<LocalDateTime>,
    String,
    Option<String>,
);

/// The `time` options as the command reads them.
pub fn time_options(m: &ArgMatches) -> Result<TimeOptions, CliFailure> {
    Ok((
        click_datetime(m, "--since", TIME)?,
        click_datetime(m, "--until", TIME)?,
        one(m, "--by").unwrap_or_else(|| "class".to_owned()),
        option_text(m, "--model"),
    ))
}

/// The `bottlenecks` options: the window and `--top` (default 15).
pub type BottleneckOptions = (Option<LocalDateTime>, Option<LocalDateTime>, usize);

/// The `bottlenecks` options as the command reads them.
pub fn bottleneck_options(m: &ArgMatches) -> Result<BottleneckOptions, CliFailure> {
    let top = number(m, "--top", 15, BOTTLENECKS)?;
    Ok((
        click_datetime(m, "--since", BOTTLENECKS)?,
        click_datetime(m, "--until", BOTTLENECKS)?,
        usize::try_from(top.max(0)).unwrap_or(usize::MAX),
    ))
}

/// The period a job's retrospective scope uses when it names none (`period_days=14`).
const SCOPE_PERIOD_DAYS: i64 = 14;

/// The retrospective options a job's scope names: `period_days` (default 14), `project`,
/// `repository` and `use_model` (default on). `since` and `until` are not read: a job payload
/// carries no instants.
pub fn scope_retrospective_options(scope: Option<&Map<String, Json>>) -> RetrospectiveOptions {
    let period_days = scope
        .and_then(|scope| scope.get("period_days"))
        .and_then(Json::as_i64)
        .unwrap_or(SCOPE_PERIOD_DAYS);
    let member = |name: &str| {
        scope
            .and_then(|scope| scope.get(name))
            .and_then(Json::as_str)
            .map(str::to_owned)
    };
    let project = member("project");
    let repository = member("repository");
    let use_model = scope
        .and_then(|scope| scope.get("use_model"))
        .and_then(Json::as_bool)
        .unwrap_or(true);
    (period_days, project, repository, use_model, None)
}
