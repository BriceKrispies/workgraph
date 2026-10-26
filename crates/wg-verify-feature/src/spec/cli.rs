//! Pure: the clap tree of `workgraph verify` and the human tables of its listings. The rows are
//! read from the wire documents the service returns, so the human output and the `--json` output
//! come from the same values, as the Python `emit` helper does.

use clap::Command;
use wg_clispec::args::{get, items, python, when};
use wg_clispec::tree::snapshot_command;
use wg_json::Value;
use wg_render::Console;

/// The `verify` command, built from the snapshot so its help and parameters match.
pub fn command() -> Command {
    snapshot_command("verify")
}

/// The first eight characters of an id (`short`), or nothing when the id is absent.
fn short(value: Option<&Value>) -> String {
    python(value.unwrap_or(&Value::Null))
        .chars()
        .take(8)
        .collect()
}

/// A table cell: a value as text, `None` (and absence) as nothing.
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(value) => python(value),
    }
}

/// `verify configs`: the configs as a table.
#[must_use]
pub fn configs_view(console: &Console, configs: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(configs))
        .iter()
        .map(|config| {
            vec![
                short(get(config, "id")),
                cell(get(config, "name")),
                cell(get(config, "scope_type")),
                short(get(config, "scope_id")),
                items(get(config, "checks")).len().to_string(),
                if matches!(get(config, "enabled"), Some(Value::Bool(true))) {
                    "yes".to_owned()
                } else {
                    "no".to_owned()
                },
                cell(get(config, "approved_by")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "id",
                "name",
                "scope",
                "scope id",
                "checks",
                "enabled",
                "approved by",
            ],
            None,
        )
        .text
}

/// `verify runs`: the runs as a table.
#[must_use]
pub fn runs_view(console: &Console, runs: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(runs))
        .iter()
        .map(|run| {
            vec![
                short(get(run, "id")),
                short(get(run, "task_id")),
                cell(get(run, "status")),
                items(get(run, "checks")).len().to_string(),
                when(get(run, "created_at")),
            ]
        })
        .collect();
    console
        .table(&rows, &["run", "task", "status", "checks", "created"], None)
        .text
}

/// `verify failures`: the failed checks as a table; the signatures are cut to 70 characters.
#[must_use]
pub fn failures_view(console: &Console, failures: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(failures))
        .iter()
        .map(|row| {
            let signatures: Vec<String> = items(get(row, "failure_signatures"))
                .iter()
                .map(python)
                .collect();
            vec![
                cell(get(row, "task_key")),
                cell(get(row, "check")),
                cell(get(row, "status")),
                cell(get(row, "exit_code")),
                signatures.join(", ").chars().take(70).collect(),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &["task", "check", "status", "exit", "signatures"],
            None,
        )
        .text
}
