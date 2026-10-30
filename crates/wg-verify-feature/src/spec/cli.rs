//! Pure: the clap tree of `workgraph verify` and the human tables of its listings. The rows are
//! read from the wire documents the service returns, so the human output and the `--json` output
//! come from the same values, as the Python `emit` helper does.

use clap::{ArgMatches, Command};
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{ReadFile, cell, get, items, many, option_text, python, when};
use wg_iface::cli::tree::snapshot_command;
use wg_json::Value;
use wg_render::Console;
use wg_types::pyval::prefix;

/// The arguments of `verify config`.
#[derive(Debug, Clone)]
pub struct ConfigArgs {
    pub name: String,
    pub scope_type: String,
    pub scope: Option<String>,
    pub checks: Vec<serde_json::Value>,
}

/// `parse_json_arg(value, "check")`: JSON text, or `@file` holding it.
fn check_document(value: &str, read_file: ReadFile<'_>) -> Result<serde_json::Value, CliFailure> {
    let source = match value.strip_prefix('@') {
        Some(path) => read_file(path).map_err(|why| CliFailure::fail(why, 1))?,
        None => value.to_owned(),
    };
    serde_json::from_str(&source)
        .map_err(|error| CliFailure::fail(format!("check is not valid JSON: {error}"), 2))
}

/// The arguments of `verify config`, with the `--check` documents parsed.
pub fn config_args(m: &ArgMatches, read_file: ReadFile<'_>) -> Result<ConfigArgs, CliFailure> {
    Ok(ConfigArgs {
        name: option_text(m, "name").unwrap_or_default(),
        scope_type: option_text(m, "--scope-type").unwrap_or_else(|| "repository".to_owned()),
        scope: option_text(m, "--scope"),
        checks: many(m, "--check")
            .iter()
            .map(|text| check_document(text, read_file))
            .collect::<Result<_, _>>()?,
    })
}

/// `verify config`: the line it prints.
#[must_use]
pub fn config_line(console: &Console, config: &Value) -> String {
    console
        .print(&format!(
            "verification config {} v{} ({} checks)",
            python(get(config, "name").unwrap_or(&Value::Null)),
            python(get(config, "version").unwrap_or(&Value::Null)),
            items(get(config, "checks")).len()
        ))
        .text
}

/// `verify run`: the verdict line and the table of its checks.
#[must_use]
pub fn run_view(console: &Console, run: &Value) -> String {
    let verdict = console.print(&format!(
        "verification {} (required passed: {})",
        python(get(run, "status").unwrap_or(&Value::Null)),
        python(get(run, "required_passed").unwrap_or(&Value::Null))
    ));
    let rows: Vec<Vec<String>> = items(get(run, "checks"))
        .iter()
        .map(|check| {
            let signatures: Vec<String> = items(get(check, "failure_signatures"))
                .iter()
                .map(python)
                .collect();
            vec![
                cell(get(check, "name")),
                cell(get(check, "kind")),
                if matches!(get(check, "required"), Some(Value::Bool(true))) {
                    "req".to_owned()
                } else {
                    "opt".to_owned()
                },
                cell(get(check, "status")),
                cell(get(check, "exit_code")),
                prefix(&signatures.join(", "), 60),
            ]
        })
        .collect();
    let checks = console.table(
        &rows,
        &["check", "kind", "", "status", "exit", "failures"],
        None,
    );
    format!("{}{}", verdict.text, checks.text)
}

/// Whether the run passed; `verify run` exits 3 when it did not.
#[must_use]
pub fn run_passed(run: &Value) -> bool {
    get(run, "status").map(python).as_deref() == Some("passed")
}

/// The `verify` command, built from the snapshot so its help and parameters match.
pub fn command() -> Command {
    snapshot_command("verify")
}

/// The first eight characters of an id (`short`), or nothing when the id is absent.
fn short(value: Option<&Value>) -> String {
    prefix(&python(value.unwrap_or(&Value::Null)), 8)
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
                prefix(&signatures.join(", "), 70),
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
