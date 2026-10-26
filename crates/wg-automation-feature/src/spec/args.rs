//! The arguments of the `automation` commands (`cli/commands/automation.py`): the clap command,
//! which is the snapshot's own tree so help and parameters match `contracts/cli.json`, and the
//! readers of the matched options, which build the documents and the requests. Functions of the
//! matches with no I/O; the handlers in [`crate::cli`] run them.

use clap::ArgMatches;
use serde_json::{Map, Value as Json};
use wg_clispec::args::{ReadFile, flag, number, one};
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{CliFailure, Snapshot, Usage};

/// Click's usage line of `automation runs`.
const RUNS: Usage = Usage {
    usage: "workgraph automation runs [OPTIONS]",
    command: "workgraph automation runs",
};

/// The clap command of the `automation` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "automation"),
        Err(_) => wg_clispec::tree::command("automation"),
    }
}

/// `parse_json_arg(value, what)`: JSON text, or `@file` holding it. `None` stays `None`.
pub fn json_arg(
    value: Option<&str>,
    what: &str,
    read_file: ReadFile<'_>,
) -> Result<Option<Json>, CliFailure> {
    let Some(value) = value else {
        return Ok(None);
    };
    let source = match value.strip_prefix('@') {
        Some(path) => read_file(path).map_err(|why| CliFailure::fail(why, 1))?,
        None => value.to_owned(),
    };
    serde_json::from_str(&source)
        .map(Some)
        .map_err(|err| CliFailure::fail(format!("{what} is not valid JSON: {err}"), 2))
}

/// The required document of `create` and `update` (`parse_json_arg(spec, "rule")`).
pub fn rule_document(
    m: &ArgMatches,
    id: &str,
    read_file: ReadFile<'_>,
) -> Result<Json, CliFailure> {
    json_arg(one(m, id).as_deref(), "rule", read_file)?
        .ok_or_else(|| CliFailure::fail("rule is not valid JSON: missing", 2))
}

/// The inputs of `trigger`: the `--input` object, with `--task` set under `task`.
pub fn trigger_inputs(
    m: &ArgMatches,
    read_file: ReadFile<'_>,
) -> Result<Map<String, Json>, CliFailure> {
    let mut inputs = match json_arg(one(m, "--input").as_deref(), "input", read_file)? {
        Some(Json::Object(map)) => map,
        Some(Json::Null) | None => Map::new(),
        Some(_) => {
            return Err(CliFailure::fail("input must be a JSON object", 2));
        }
    };
    if let Some(task) = one(m, "--task").filter(|text| !text.is_empty()) {
        inputs.insert("task".to_owned(), Json::String(task));
    }
    Ok(inputs)
}

/// The inputs of `preview`: `{"task": ...}` when `--task` is given.
pub fn preview_inputs(m: &ArgMatches) -> Map<String, Json> {
    match one(m, "--task").filter(|text| !text.is_empty()) {
        Some(task) => Map::from_iter([("task".to_owned(), Json::String(task))]),
        None => Map::new(),
    }
}

/// The `--spec` of `preview`, a rule document when given.
pub fn preview_spec(m: &ArgMatches, read_file: ReadFile<'_>) -> Result<Option<Json>, CliFailure> {
    json_arg(one(m, "--spec").as_deref(), "spec", read_file)
}

/// The `--rule`, `--status` and `--limit` of `runs`.
pub fn runs_args(m: &ArgMatches) -> Result<(Option<String>, Option<String>, i64), CliFailure> {
    Ok((
        one(m, "--rule"),
        one(m, "--status"),
        number(m, "--limit", 50, RUNS)?,
    ))
}

/// The `--unread` of `notifications`.
#[must_use]
pub fn unread_only(m: &ArgMatches) -> bool {
    flag(m, "--unread")
}

/// The positional name argument (`name`) of the commands that take one.
#[must_use]
pub fn name_arg(m: &ArgMatches) -> String {
    one(m, "name").unwrap_or_default()
}

/// The `--event` and the `--rule` of `preview`.
#[must_use]
pub fn preview_targets(m: &ArgMatches) -> (Option<String>, Option<String>) {
    (one(m, "--rule"), one(m, "--event"))
}
