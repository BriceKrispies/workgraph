//! The pure parts of the data commands (`cli/commands/data.py`): the clap commands, the readers of
//! their matches, the documents they print and the human views. Nothing here reads a file or the
//! store. [`crate::service`] runs the I/O and [`crate::cli`] prints what these build.

use std::path::PathBuf;

use clap::{ArgMatches, Command};
use serde_json::Value as Json;
use wg_iface::cli::args::{flag, one, python};
use wg_iface::cli::tree::snapshot_command;
use wg_json::{PyStr, Value, dict};
use wg_render::{Console, escape};
use wg_wire::convention::text;

/// The clap command of `backup`, from the snapshot so help and parameters match `contracts/cli.json`.
#[must_use]
pub fn backup_command() -> Command {
    snapshot_command("backup")
}

/// The clap command of `restore`.
#[must_use]
pub fn restore_command() -> Command {
    snapshot_command("restore")
}

/// The clap command of `import`.
#[must_use]
pub fn import_command() -> Command {
    snapshot_command("import")
}

/// The clap command of the `export` group (`workspace`, `analytics`).
#[must_use]
pub fn export_command() -> Command {
    snapshot_command("export")
}

/// `--output` of `backup`, when given.
#[must_use]
pub fn backup_output(m: &ArgMatches) -> Option<PathBuf> {
    one(m, "--output").map(PathBuf::from)
}

/// `--include-artifacts` of `backup`.
#[must_use]
pub fn include_artifacts(m: &ArgMatches) -> bool {
    flag(m, "--include-artifacts")
}

/// A required positional path (`archive`, `output`, `source`).
#[must_use]
pub fn path_arg(m: &ArgMatches, id: &str) -> PathBuf {
    PathBuf::from(one(m, id).unwrap_or_default())
}

/// `--with-config` of `restore`.
#[must_use]
pub fn with_config(m: &ArgMatches) -> bool {
    flag(m, "--with-config")
}

/// `--force` of `restore`.
#[must_use]
pub fn force(m: &ArgMatches) -> bool {
    flag(m, "--force")
}

/// `-w/--workspace` of `export workspace`, when given (the global default applies otherwise).
#[must_use]
pub fn workspace_arg(m: &ArgMatches) -> Option<String> {
    one(m, "--workspace")
}

/// The `backup` result: `{"path", "bytes", **manifest}` (`persistence/backup.py` `create_backup`).
#[must_use]
pub fn backup_document(
    path: &str,
    bytes: u64,
    stamp: &str,
    version: &str,
    schema_revision: Option<&str>,
    include_artifacts: bool,
) -> Value {
    dict!({
        "path": text(path),
        "bytes": Value::Int(i128::from(bytes)),
        "format": text(wg_archivespec::BACKUP_FORMAT),
        "created_at": text(stamp),
        "workgraph_version": text(version),
        "schema_revision": opt_text(schema_revision),
        "includes_artifacts": Value::Bool(include_artifacts),
    })
}

/// The `restore` result (`restore_backup`'s dict).
#[must_use]
pub fn restore_document(
    restored_from: &str,
    manifest: &Json,
    safety_copy: Option<&str>,
    schema_before: Option<&str>,
    schema_after: Option<&str>,
) -> Value {
    dict!({
        "restored_from": text(restored_from),
        "manifest": from_json(manifest),
        "safety_copy": opt_text(safety_copy),
        "schema_before": opt_text(schema_before),
        "schema_after": opt_text(schema_after),
    })
}

/// The `export workspace` result: the file and the rows per table, in the order they were written.
#[must_use]
pub fn export_document(path: &str, tables: &[(String, usize)]) -> Value {
    dict!({ "path": text(path), "tables": counts(tables) })
}

/// The `import` result: the workspace id and the rows per table, in the order they were read.
#[must_use]
pub fn import_document(workspace_id: &str, tables: &[(String, usize)]) -> Value {
    dict!({
        "workspace_id": text(workspace_id),
        "tables": counts(tables),
    })
}

/// The human line of `backup`: `backup written to ... (N bytes, schema R)`.
#[must_use]
pub fn backup_view(
    console: &Console,
    path: &str,
    bytes: u64,
    schema_revision: Option<&str>,
) -> String {
    console
        .print(&format!(
            "backup written to {} ({bytes} bytes, schema {})",
            escape(path),
            schema_revision.unwrap_or("None")
        ))
        .text
}

/// The human view of `restore`: a bold `restored` title and each non-empty field of the result.
#[must_use]
pub fn restore_view(console: &Console, document: &Value) -> String {
    let mut out = console.print("[bold]restored[/bold]").text;
    if let Value::Dict(entries) = document {
        for (key, value) in entries {
            if is_empty(value) {
                continue;
            }
            out.push_str(
                &console
                    .print(&format!(
                        "  [cyan]{}[/cyan]: {}",
                        key_text(key),
                        python(value)
                    ))
                    .text,
            );
        }
    }
    out
}

/// The human view of `export workspace`: the path, then the rows per table.
#[must_use]
pub fn export_view(console: &Console, path: &str, tables: &[(String, usize)]) -> String {
    let mut out = console.print(&format!("exported to {}", escape(path))).text;
    out.push_str(&tables_table(console, tables));
    out
}

/// The human view of `import`: the workspace id, then the rows per table.
#[must_use]
pub fn import_view(console: &Console, workspace_id: &str, tables: &[(String, usize)]) -> String {
    let mut out = console
        .print(&format!("imported workspace {}", escape(workspace_id)))
        .text;
    out.push_str(&tables_table(console, tables));
    out
}

/// `table(sorted(tables.items()), ["table", "rows"])`.
fn tables_table(console: &Console, tables: &[(String, usize)]) -> String {
    let mut sorted: Vec<&(String, usize)> = tables.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<Vec<String>> = sorted
        .iter()
        .map(|(name, count)| vec![name.clone(), count.to_string()])
        .collect();
    console.table(&rows, &["table", "rows"], None).text
}

/// A Python dict of the counts, in the given order.
fn counts(tables: &[(String, usize)]) -> Value {
    Value::Dict(
        tables
            .iter()
            .map(|(name, count)| {
                (
                    PyStr::from(name.as_str()),
                    Value::Int(i128::try_from(*count).unwrap_or(i128::MAX)),
                )
            })
            .collect(),
    )
}

/// `None` as JSON `null`, text otherwise.
fn opt_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

/// `common.kv` leaves out a `None`, an empty list and an empty dict.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::List(items) => items.is_empty(),
        Value::Dict(entries) => entries.is_empty(),
        _ => false,
    }
}

/// A key of a Python dict as text.
fn key_text(key: &PyStr) -> String {
    key.code_points()
        .iter()
        .filter_map(|point| char::from_u32(*point))
        .collect()
}

/// A JSON document read from a file, as the output value of the same Python dict (object keys in
/// file order, numbers as integers when they are).
#[must_use]
pub fn from_json(value: &Json) -> Value {
    match value {
        Json::Null => Value::Null,
        Json::Bool(flag) => Value::Bool(*flag),
        Json::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(int), _) => Value::Int(i128::from(int)),
            (None, Some(float)) => Value::Float(float),
            (None, None) => Value::Null,
        },
        Json::String(string) => text(string),
        Json::Array(items) => Value::List(items.iter().map(from_json).collect()),
        Json::Object(map) => Value::Dict(
            map.iter()
                .map(|(key, member)| (PyStr::from(key.as_str()), from_json(member)))
                .collect(),
        ),
    }
}

/// `--dataset` of `export analytics` (default `task_facts`).
#[must_use]
pub fn analytics_dataset(m: &ArgMatches) -> String {
    one(m, "--dataset").unwrap_or_else(|| "task_facts".to_owned())
}

/// `--format` of `export analytics` (default `csv`), as typed; the caller parses it.
#[must_use]
pub fn analytics_format(m: &ArgMatches) -> String {
    one(m, "--format").unwrap_or_else(|| "csv".to_owned())
}

/// The `export analytics` result (`AnalyticsService.export`): path, format, dataset and bytes.
#[must_use]
pub fn export_analytics_document(path: &str, format: &str, dataset: &str, bytes: u64) -> Value {
    dict!({
        "path": text(path),
        "format": text(format),
        "dataset": text(dataset),
        "bytes": Value::Int(i128::from(bytes)),
    })
}

/// The human line of `export analytics`: `wrote <path>`.
#[must_use]
pub fn export_analytics_view(console: &Console, path: &str) -> String {
    console.print(&format!("wrote {}", escape(path))).text
}
