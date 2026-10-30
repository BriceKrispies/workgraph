//! wg-archivespec: the pure half of the archive formats (SH-20).
//!
//! * The backup manifest, archive entry names and the stamp in file names (F10).
//! * The workspace export layout and the JSON Lines it writes and reads (F14). Each record is one
//!   `{"table": ..., "row": {...}}` object in `persistence/backup.py`'s foreign-key order, encoded
//!   with Python's `json.dumps` defaults; the layout is generated from the ORM metadata in
//!   [`layout`].
//! * The analytics dataset encodings: CSV and JSON as DuckDB writes them, and the column types
//!   the Parquet writer uses (F15, DV6).
//!
//! Nothing here touches the filesystem, the clock or a database connection. The shell crate
//! `wg-archive` reads and writes the bytes and runs the statements these functions describe.

mod layout;
mod parquet_out;

pub use parquet_out::parquet_bytes;

use std::path::Path;

use serde_json::Value as JsonValue;
use wg_json::{PyStr, Value, cli_json, db_json, dict, log_json};
use wg_rows::{Raw, Statement};
use wg_types::{Timestamp, WgError};

pub use layout::EXPORT_TABLES;

/// `manifest.json`'s `format` field.
pub const BACKUP_FORMAT: &str = "workgraph-backup-v1";

/// The first line's `format` field of a workspace export.
pub const EXPORT_FORMAT: &str = "workgraph-workspace-export-v1";

/// How a column's value is written in an export record (the `persistence/types.py` decorators).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Any other column: the stored value as it is.
    Plain,
    /// `UTCDateTime`: a stored O11 string, exported as `datetime.isoformat()`.
    Ts,
    /// `JSONText`: a stored O10 string, exported as the parsed value.
    Json,
    /// `Boolean`: a stored integer, exported as `true` or `false`.
    Bool,
}

/// Which rows of a table a workspace export takes (`_scoped_select`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The table has a `workspace_id` column.
    Workspace,
    /// The `workspaces` table itself.
    Workspaces,
    /// The first `(column, parent)` whose parent table was exported takes the rows whose column
    /// is one of the parent's exported ids.
    Parents(&'static [(&'static str, &'static str)]),
    /// No scope: the table is skipped because no parent exports ids.
    Unscoped,
    /// `SKIP_EXPORT`: never exported.
    Skipped,
}

/// One table of the export layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportTable {
    pub name: &'static str,
    pub scope: Scope,
    /// The primary key columns. A table with one key column contributes its ids to its children.
    pub pk: &'static [&'static str],
    pub columns: &'static [(&'static str, Kind)],
}

/// The layout entry of `name`.
pub fn table(name: &str) -> Option<&'static ExportTable> {
    EXPORT_TABLES.iter().find(|table| table.name == name)
}

/// The ids a child table's scope can filter on: each exported single-key parent, by table name,
/// in export order. A parent with a composite key contributes an empty list (as Python's does).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportedIds {
    parents: Vec<(&'static str, Vec<String>)>,
}

impl ExportedIds {
    /// Returns the ids with one more exported table recorded after the ones already held.
    pub fn record(mut self, table: &'static str, ids: Vec<String>) -> Self {
        self.parents.push((table, ids));
        self
    }

    fn of(&self, table: &str) -> Option<&[String]> {
        self.parents
            .iter()
            .find(|(name, _)| *name == table)
            .map(|(_, ids)| ids.as_slice())
    }
}

/// The statement that reads one table's rows for a workspace export, or `None` when the table is
/// skipped. Columns come in layout order, which is the order `_row_dict` reads them in.
pub fn export_select(
    table: &ExportTable,
    workspace_id: &str,
    exported: &ExportedIds,
) -> Option<Statement> {
    let columns = table
        .columns
        .iter()
        .map(|(name, _)| quote(name))
        .collect::<Vec<_>>()
        .join(", ");
    let from = format!("SELECT {columns} FROM {}", quote(table.name));
    let text = |value: &str| Raw::Text(value.to_string());
    match table.scope {
        Scope::Skipped | Scope::Unscoped => None,
        Scope::Workspace => Some(Statement {
            sql: format!("{from} WHERE {} = ?1", quote("workspace_id")),
            params: vec![text(workspace_id)],
        }),
        Scope::Workspaces => Some(Statement {
            sql: format!("{from} WHERE {} = ?1", quote("id")),
            params: vec![text(workspace_id)],
        }),
        Scope::Parents(candidates) => candidates.iter().find_map(|(column, parent)| {
            let ids = exported.of(parent)?;
            // Python passes `[""]` when the parent's id set is empty, which matches nothing.
            let ids: Vec<&str> = if ids.is_empty() {
                vec![""]
            } else {
                ids.iter().map(String::as_str).collect()
            };
            Some(Statement {
                sql: format!(
                    "{from} WHERE {} IN (SELECT value FROM json_each(?1))",
                    quote(column)
                ),
                params: vec![Raw::Text(json_array(&ids))],
            })
        }),
    }
}

/// The JSON array text of `ids`, the single parameter of an `IN (SELECT ... json_each(...))`.
fn json_array(ids: &[&str]) -> String {
    let items: Vec<JsonValue> = ids
        .iter()
        .map(|id| JsonValue::String((*id).to_string()))
        .collect();
    JsonValue::Array(items).to_string()
}

/// Quotes an identifier. Table and column names are plain ASCII, so this only guards keywords.
fn quote(name: &str) -> String {
    format!("\"{name}\"")
}

/// The line terminator of an export file. Python opens export files in text mode, so Windows
/// writes `\r\n`; the caller passes `cfg!(windows)`.
pub fn line_end(windows: bool) -> &'static str {
    if windows { "\r\n" } else { "\n" }
}

/// The first line of a workspace export: `json.dumps({format, workspace_id, workgraph_version})`.
pub fn export_header(workspace_id: &str, version: &str, windows: bool) -> Result<String, WgError> {
    let value = dict!({
        "format": Value::Str(key(EXPORT_FORMAT)),
        "workspace_id": Value::Str(key(workspace_id)),
        "workgraph_version": Value::Str(key(version)),
    });
    let line = log_json(&value).map_err(dump_failure)?;
    Ok(format!("{line}{}", line_end(windows)))
}

/// The export line of one row: `json.dumps({"table": name, "row": {...}})` without the terminator.
/// `row` holds the values in the order of `table.columns`.
pub fn export_line(table: &ExportTable, row: &[Raw]) -> Result<String, WgError> {
    if row.len() != table.columns.len() {
        return Err(WgError::validation(format!(
            "row of {} has {} values, expected {}",
            table.name,
            row.len(),
            table.columns.len()
        )));
    }
    let mut fields = Vec::with_capacity(row.len());
    for ((name, kind), raw) in table.columns.iter().zip(row) {
        fields.push((key(name), export_value(*kind, raw)?));
    }
    let value = dict!({
        "table": Value::Str(key(table.name)),
        "row": Value::Dict(fields),
    });
    log_json(&value).map_err(dump_failure)
}

/// How one stored value is written in a record.
fn export_value(kind: Kind, raw: &Raw) -> Result<Value, WgError> {
    Ok(match (kind, raw) {
        (_, Raw::Null) => Value::Null,
        (_, Raw::Blob(bytes)) => bytes_value(bytes),
        (Kind::Json, Raw::Text(text)) => {
            let parsed: JsonValue = serde_json::from_str(text)
                .map_err(|err| WgError::validation(format!("stored JSON does not parse: {err}")))?;
            to_py(&parsed)
        }
        (Kind::Ts, Raw::Text(text)) => {
            Value::Str(key(&Timestamp::parse_iso(text)?.format_isoformat()))
        }
        (Kind::Bool, Raw::Int(flag)) => Value::Bool(*flag != 0),
        (_, Raw::Int(number)) => Value::Int(i128::from(*number)),
        (_, Raw::Real(number)) => Value::Float(*number),
        (_, Raw::Text(text)) => Value::Str(key(text)),
    })
}

/// A parsed export line.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportRow {
    pub table: &'static ExportTable,
    /// The bound values, in the order of the record's fields.
    pub columns: Vec<&'static str>,
    pub params: Vec<Raw>,
}

impl ImportRow {
    /// The `INSERT` of this row.
    pub fn statement(&self) -> Statement {
        let names = self
            .columns
            .iter()
            .map(|name| quote(name))
            .collect::<Vec<_>>()
            .join(", ");
        let holes = (1..=self.columns.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        Statement {
            sql: format!(
                "INSERT INTO {} ({names}) VALUES ({holes})",
                quote(self.table.name)
            ),
            params: self.params.clone(),
        }
    }
}

/// The workspace id on an export's first line. A line that is not an export header, or whose
/// format is not [`EXPORT_FORMAT`], is refused.
pub fn parse_header(line: &str) -> Result<String, WgError> {
    let value: JsonValue = serde_json::from_str(line)
        .map_err(|_| WgError::validation("not a Workgraph workspace export".to_string()))?;
    if value.get("format").and_then(JsonValue::as_str) != Some(EXPORT_FORMAT) {
        return Err(WgError::validation(
            "not a Workgraph workspace export".to_string(),
        ));
    }
    value
        .get("workspace_id")
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .ok_or_else(|| WgError::validation("not a Workgraph workspace export".to_string()))
}

/// Parses one record of an export, converting each value the way `import_workspace` binds it.
pub fn parse_row(line: &str) -> Result<ImportRow, WgError> {
    let value: JsonValue = serde_json::from_str(line)
        .map_err(|err| WgError::validation(format!("export record does not parse: {err}")))?;
    let name = value
        .get("table")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| WgError::validation("export record has no table".to_string()))?;
    let table = table(name).ok_or_else(|| WgError::validation(format!("unknown table {name}")))?;
    let fields = value
        .get("row")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| WgError::validation("export record has no row".to_string()))?;
    let mut columns = Vec::with_capacity(fields.len());
    let mut params = Vec::with_capacity(fields.len());
    for (column, item) in fields {
        let (name, kind) = table
            .columns
            .iter()
            .find(|(candidate, _)| *candidate == column.as_str())
            .ok_or_else(|| {
                WgError::validation(format!("unknown column {}.{column}", table.name))
            })?;
        columns.push(*name);
        params.push(import_value(*kind, item)?);
    }
    Ok(ImportRow {
        table,
        columns,
        params,
    })
}

/// The stored form of one imported value (`import_workspace`'s conversions, then the column's
/// bind processor).
fn import_value(kind: Kind, item: &JsonValue) -> Result<Raw, WgError> {
    if let Some(bytes) = bytes_of(item) {
        return Ok(Raw::Blob(bytes));
    }
    Ok(match (kind, item) {
        (_, JsonValue::Null) => Raw::Null,
        (Kind::Ts, JsonValue::String(text)) => Raw::Text(Timestamp::parse_iso(text)?.format_db()),
        (Kind::Json, value) => match db_json(&to_py(value)).map_err(dump_failure)? {
            Some(text) => Raw::Text(text),
            None => Raw::Null,
        },
        (_, JsonValue::Bool(flag)) => Raw::Int(i64::from(*flag)),
        (_, JsonValue::Number(number)) => match (number.as_i64(), number.as_f64()) {
            (Some(int), _) => Raw::Int(int),
            (None, Some(real)) => Raw::Real(real),
            (None, None) => {
                return Err(WgError::validation(format!(
                    "number {number} is out of range"
                )));
            }
        },
        (_, JsonValue::String(text)) => Raw::Text(text.clone()),
        (_, other) => {
            return Err(WgError::validation(format!(
                "a {kind:?} column cannot hold {other}"
            )));
        }
    })
}

/// The `{"__bytes__": hex}` form of a bytes value, when `item` has it.
fn bytes_of(item: &JsonValue) -> Option<Vec<u8>> {
    let object = item.as_object()?;
    let hex = object.get("__bytes__")?.as_str()?;
    if object.len() != 1 {
        return None;
    }
    decode_hex(hex)
}

fn bytes_value(bytes: &[u8]) -> Value {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    dict!({ "__bytes__": Value::Str(key(&hex)) })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

/// The `workgraph-backup-v1` manifest: `json.dumps(manifest, indent=2)`.
pub fn manifest_text(
    created_at: &str,
    version: &str,
    schema_revision: Option<&str>,
    includes_artifacts: bool,
) -> Result<String, WgError> {
    let value = dict!({
        "format": Value::Str(key(BACKUP_FORMAT)),
        "created_at": Value::Str(key(created_at)),
        "workgraph_version": Value::Str(key(version)),
        "schema_revision": schema_revision.map_or(Value::Null, |revision| Value::Str(key(revision))),
        "includes_artifacts": Value::Bool(includes_artifacts),
    });
    let mut text = cli_json(&value).map_err(dump_failure)?;
    text.pop();
    Ok(text)
}

/// The stamp of a backup: `%Y%m%dT%H%M%SZ` of a UTC timestamp.
pub fn stamp(at: Timestamp) -> String {
    // `format_db` is `YYYY-MM-DDTHH:MM:SS.ffffff+00:00`; the first 19 bytes are the stamp's fields.
    at.format_db()[..19]
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .chain(std::iter::once('Z'))
        .collect()
}

/// The default backup file name under `<data>/backups/`.
pub fn backup_file_name(stamp: &str) -> String {
    format!("workgraph-backup-{stamp}.zip")
}

/// The name of the safety copy a restore keeps under `<data>/backups/`.
pub fn pre_restore_file_name(stamp: &str) -> String {
    format!("pre-restore-{stamp}.db")
}

/// Whether an archive entry name stays inside the extraction directory: relative, with no drive
/// prefix and no `..` component, under either separator.
pub fn is_safe_entry(name: &str) -> bool {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains(':') {
        return false;
    }
    name.split(['/', '\\']).all(|part| part != "..")
}

/// The fields a restore reads from a manifest, or the reason the archive is not a backup.
pub fn parse_manifest(text: &str) -> Result<JsonValue, WgError> {
    let value: JsonValue = serde_json::from_str(text)
        .map_err(|err| WgError::validation(format!("manifest.json does not parse: {err}")))?;
    Ok(value)
}

/// The `schema_revision` a manifest records, if any.
pub fn manifest_revision(manifest: &JsonValue) -> Option<String> {
    manifest
        .get("schema_revision")
        .and_then(JsonValue::as_str)
        .map(str::to_string)
}

/// The `workspace_exists` probe of an import: one row when the workspace is already present.
pub fn workspace_exists(workspace_id: &str) -> Statement {
    Statement {
        sql: "SELECT 1 FROM \"workspaces\" WHERE \"id\" = ?1".to_string(),
        params: vec![Raw::Text(workspace_id.to_string())],
    }
}

/// The live-worker probe of a restore (`restore_backup`). A failing probe counts as no worker.
pub fn live_workers() -> Statement {
    Statement {
        sql:
            "SELECT COUNT(*) FROM \"workers\" WHERE \"status\" = 'running' AND \"heartbeat_at\" > \
              strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '-60 seconds')"
                .to_string(),
        params: Vec::new(),
    }
}

/// The ids of a table's rows, for the children's scopes: the first column of a single-key table.
pub fn ids_of(table: &ExportTable, rows: &[Vec<Raw>]) -> Vec<String> {
    if table.pk.len() != 1 {
        return Vec::new();
    }
    let Some(index) = table
        .columns
        .iter()
        .position(|(name, _)| *name == table.pk[0])
    else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| match row.get(index) {
            Some(Raw::Text(id)) => Some(id.clone()),
            Some(Raw::Int(number)) => Some(number.to_string()),
            _ => None,
        })
        .collect()
}

/// The analytics datasets: a column list and rows of stored values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dataset {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Raw>>,
    /// The indexes of the columns DuckDB types as BOOLEAN. Their `Int` cells (0 or 1) are written as
    /// `true` and `false`, as DuckDB writes them.
    pub boolean_columns: Vec<usize>,
}

/// The analytics export formats (`analytics export --format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Parquet,
}

/// Parses `--format`; anything else is refused with the Python message.
pub fn parse_format(text: &str) -> Result<Format, WgError> {
    match text {
        "csv" => Ok(Format::Csv),
        "json" => Ok(Format::Json),
        "parquet" => Ok(Format::Parquet),
        _ => Err(WgError::validation(
            "format must be csv, json or parquet".to_string(),
        )),
    }
}

/// The type a Parquet column gets: the Python `_arrow_table` inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Int64,
    Float64,
    Utf8,
}

/// The type of each column: integers when every value is an integer or null (and one is not
/// null), floats when every value is numeric and one is a float, text otherwise.
pub fn column_types(dataset: &Dataset) -> Vec<ColumnType> {
    (0..dataset.columns.len())
        .map(|index| {
            let values: Vec<&Raw> = dataset
                .rows
                .iter()
                .filter_map(|row| row.get(index))
                .collect();
            let all_int = values.iter().all(|v| matches!(v, Raw::Int(_) | Raw::Null));
            let any_value = values.iter().any(|v| !matches!(v, Raw::Null));
            let numeric = values
                .iter()
                .all(|v| matches!(v, Raw::Int(_) | Raw::Real(_) | Raw::Null));
            let any_real = values.iter().any(|v| matches!(v, Raw::Real(_)));
            if all_int && any_value {
                ColumnType::Int64
            } else if numeric && any_real {
                ColumnType::Float64
            } else {
                ColumnType::Utf8
            }
        })
        .collect()
}

/// A stored value as text for CSV and JSON (`str` of the Python value).
pub(crate) fn cell_text(raw: &Raw) -> Option<String> {
    match raw {
        Raw::Null => None,
        Raw::Int(number) => Some(number.to_string()),
        Raw::Real(number) => Some(py_repr(*number)),
        Raw::Text(text) => Some(text.clone()),
        Raw::Blob(bytes) => Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
    }
}

/// Python's `repr` of a float: the shortest text that round-trips.
fn py_repr(number: f64) -> String {
    wg_json::py_float_repr(number)
}

/// The file bytes of a dataset in `format`: the CSV text, the JSON text or the Parquet file.
pub fn dataset_bytes(dataset: &Dataset, format: Format) -> Result<Vec<u8>, WgError> {
    Ok(match format {
        Format::Csv => csv_text(dataset).into_bytes(),
        Format::Json => json_text(dataset)?.into_bytes(),
        Format::Parquet => parquet_bytes(dataset)?,
    })
}

/// The dataset as CSV with a header row (DuckDB's `FORMAT CSV, HEADER`): `,` separators, `\n`
/// terminators, NULL as an empty field, and a field quoted when it holds a separator, a quote or
/// a line break, with inner quotes doubled.
pub fn csv_text(dataset: &Dataset) -> String {
    let mut out = String::new();
    let header: Vec<String> = dataset
        .columns
        .iter()
        .map(|name| csv_field(Some(name)))
        .collect();
    out.push_str(&header.join(","));
    out.push('\n');
    for row in &dataset.rows {
        let fields: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(index, raw)| csv_field(cell(dataset, index, raw).as_deref()))
            .collect();
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// A cell as CSV and JSON text: a boolean column writes `true` or `false`, other cells their text.
fn cell(dataset: &Dataset, index: usize, raw: &Raw) -> Option<String> {
    match raw {
        Raw::Int(flag) if dataset.boolean_columns.contains(&index) => {
            Some((*flag != 0).to_string())
        }
        other => cell_text(other),
    }
}

fn csv_field(text: Option<&str>) -> String {
    match text {
        None => String::new(),
        Some("") => "\"\"".to_string(),
        Some(text) if text.contains([',', '"', '\n', '\r']) => {
            format!("\"{}\"", text.replace('"', "\"\""))
        }
        Some(text) => text.to_string(),
    }
}

/// The dataset as a JSON array, one tab-indented object per line (DuckDB's `FORMAT JSON,
/// ARRAY TRUE`). Floats are written in fixed notation with the shortest round-trip digits, as
/// DuckDB's JSON writer does (`0.00001`, `100000000000000000000.0`).
pub fn json_text(dataset: &Dataset) -> Result<String, WgError> {
    if dataset.rows.is_empty() {
        return Ok("[\n\t\n]\n".to_string());
    }
    let mut records = Vec::with_capacity(dataset.rows.len());
    for row in &dataset.rows {
        let mut fields = Vec::with_capacity(dataset.columns.len());
        for (index, (name, raw)) in dataset.columns.iter().zip(row).enumerate() {
            let value = match raw {
                Raw::Null => "null".to_string(),
                Raw::Int(flag) if dataset.boolean_columns.contains(&index) => {
                    (*flag != 0).to_string()
                }
                Raw::Int(number) => number.to_string(),
                Raw::Real(number) => fixed_float(*number).unwrap_or_else(|| "null".to_string()),
                other => json_string(&cell_text(other).unwrap_or_default())?,
            };
            fields.push(format!("{}:{value}", json_string(name)?));
        }
        records.push(format!("\t{{{}}}", fields.join(",")));
    }
    Ok(format!("[\n{}\n]\n", records.join(",\n")))
}

/// A JSON string literal with the escapes JSON requires; non-ASCII text stays as it is.
fn json_string(text: &str) -> Result<String, WgError> {
    serde_json::to_string(text)
        .map_err(|err| WgError::validation(format!("cannot encode text: {err}")))
}

/// A finite float in fixed notation: the shortest digits that round-trip, with `.0` when the value
/// is whole. Non-finite values have no JSON number.
fn fixed_float(number: f64) -> Option<String> {
    if !number.is_finite() {
        return None;
    }
    let text = format!("{number}");
    Some(if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    })
}

/// `PRAGMA integrity_check`: its first row's text (`ok` for a sound database).
pub fn integrity_check() -> Statement {
    Statement {
        sql: "PRAGMA integrity_check".to_string(),
        params: Vec::new(),
    }
}

/// The text of the first column of the first row, when that value is text.
pub fn first_text(rows: &[Vec<Raw>]) -> Option<String> {
    match rows.first()?.first()? {
        Raw::Text(text) => Some(text.clone()),
        _ => None,
    }
}

/// The integer in the first column of the first row, when that value is one.
pub fn first_int(rows: &[Vec<Raw>]) -> Option<i64> {
    match rows.first()?.first()? {
        Raw::Int(number) => Some(*number),
        _ => None,
    }
}

/// The ids of the workspace's tasks, which an import re-indexes for search.
pub fn task_ids_in(workspace_id: &str) -> Statement {
    Statement {
        sql: "SELECT \"id\" FROM \"tasks\" WHERE \"workspace_id\" = ?1 ORDER BY \"number\""
            .to_string(),
        params: vec![Raw::Text(workspace_id.to_string())],
    }
}

/// The SQLite side files of a database (`-wal` and `-shm`), which a restore removes.
pub fn sidecar_files(database: &Path) -> Vec<std::path::PathBuf> {
    ["-wal", "-shm"]
        .iter()
        .map(|suffix| std::path::PathBuf::from(format!("{}{suffix}", database.display())))
        .collect()
}

/// The error of a failed file operation: `cannot <action> <path>: <err>`.
pub fn io_failure(action: &str, path: &Path, err: &std::io::Error) -> WgError {
    WgError::workgraph(format!("cannot {action} {}: {err}", path.display()))
}

/// The error of a failed zip write to `path`.
pub fn zip_failure(path: &Path, err: &str) -> WgError {
    WgError::workgraph(format!("cannot write {}: {err}", path.display()))
}

/// The last component of a path as text (empty when it has none).
pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a path names a `.toml` file.
pub fn is_toml(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "toml")
}

/// The archive name of `file` under `base`: its components joined with `/`.
pub fn entry_name(base: &Path, file: &Path) -> String {
    file.strip_prefix(base)
        .unwrap_or(file)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn key(text: &str) -> PyStr {
    PyStr::from_code_points(text.chars().map(u32::from).collect())
}

fn to_py(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(flag) => Value::Bool(*flag),
        JsonValue::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(int), _) => Value::Int(i128::from(int)),
            (None, Some(real)) => Value::Float(real),
            (None, None) => Value::Null,
        },
        JsonValue::String(text) => Value::Str(key(text)),
        JsonValue::Array(items) => Value::List(items.iter().map(to_py).collect()),
        JsonValue::Object(map) => Value::Dict(
            map.iter()
                .map(|(name, item)| (key(name), to_py(item)))
                .collect(),
        ),
    }
}

fn dump_failure(error: wg_json::DumpError) -> WgError {
    WgError::workgraph(format!("cannot serialise a value: {}", error.python_name()))
}
