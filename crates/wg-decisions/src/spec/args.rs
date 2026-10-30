//! The arguments of the `decision` commands (`cli/commands/decisions.py`): the clap command, which
//! is the snapshot's own tree so help and parameters match `contracts/cli.json`, and the readers of
//! the matched options, which build the requests. Functions of the matches, with no I/O; the
//! handler in [`crate::cli`] runs them.

use clap::ArgMatches;
use serde_json::{Value as Json, json};
use wg_iface::cli::args::{many, number, one};
use wg_iface::cli::tree::{SNAPSHOT_JSON, placeholder};
use wg_iface::cli::{CliFailure, Snapshot, Usage};

/// The usage of `decision list` and `decision request`, as Click prints them in a usage error.
const LIST: Usage = Usage {
    usage: "workgraph decision list [OPTIONS]",
    command: "workgraph decision list",
};
const REQUEST: Usage = Usage {
    usage: "workgraph decision request [OPTIONS] {task}",
    command: "workgraph decision request",
};

/// The statuses `decision list --status` takes.
const STATUSES: [&str; 4] = ["open", "decided", "withdrawn", "all"];

/// The clap command of the `decision` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "decision"),
        Err(_) => wg_iface::cli::tree::command("decision"),
    }
}

/// The `--status` and `--limit` of `decision list`.
pub fn list_args(m: &ArgMatches) -> Result<(String, i64), CliFailure> {
    let status = one(m, "--status").unwrap_or_else(|| "open".to_owned());
    if !STATUSES.contains(&status.as_str()) {
        return Err(CliFailure::fail(
            "--status must be open, decided, withdrawn or all",
            2,
        ));
    }
    Ok((status, number(m, "--limit", 50, LIST)?))
}

/// `_parse_option`: `label` or `label::detail`, as the request option it makes.
fn parse_option(spec: &str, recommended: bool) -> Json {
    let (label, detail) = spec.split_once("::").unwrap_or((spec, ""));
    json!({"label": label.trim(), "detail": detail.trim(), "recommended": recommended})
}

/// The task, question and options of `decision request`.
pub fn request_args(m: &ArgMatches) -> Result<(String, String, Vec<Json>), CliFailure> {
    let specs = many(m, "--option");
    let recommend = one(m, "--recommend")
        .map(|_| number(m, "--recommend", 0, REQUEST))
        .transpose()?;
    let count = i64::try_from(specs.len()).unwrap_or(i64::MAX);
    if recommend.is_some_and(|position| !(1..=count).contains(&position)) {
        return Err(CliFailure::fail(
            format!("--recommend must be between 1 and {count}"),
            2,
        ));
    }
    let options = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let position = i64::try_from(index + 1).unwrap_or(i64::MAX);
            parse_option(spec, recommend == Some(position))
        })
        .collect();
    Ok((
        one(m, "task").unwrap_or_default(),
        one(m, "--question").unwrap_or_default(),
        options,
    ))
}

/// The option id, free text and note of `decision choose`.
#[must_use]
pub fn choose_args(m: &ArgMatches) -> [Option<String>; 3] {
    [one(m, "--option"), one(m, "--other"), one(m, "--note")]
}

/// The decision id argument of `show`, `choose` and `withdraw`.
#[must_use]
pub fn decision_ref(m: &ArgMatches) -> String {
    one(m, "decision_id").unwrap_or_default()
}

/// The `--reason` of `decision withdraw`.
#[must_use]
pub fn reason(m: &ArgMatches) -> Option<String> {
    one(m, "--reason")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_split_the_label_from_the_detail() {
        assert_eq!(
            parse_option(" SQLite :: embedded ", true),
            json!({"label": "SQLite", "detail": "embedded", "recommended": true})
        );
        assert_eq!(
            parse_option("Plain", false),
            json!({"label": "Plain", "detail": "", "recommended": false})
        );
    }
}
