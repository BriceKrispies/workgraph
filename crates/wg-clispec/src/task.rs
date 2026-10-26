//! The pure half of the `task` commands (`cli/commands/task.py`): the request bodies the
//! options make and the human views of the results.
//!
//! The handlers in `wg-cli` read nothing but the matched options; everything that decides what
//! is asked, or how a result looks, is here as a function of its inputs.

use clap::ArgMatches;
use serde_json::{Map, Value as Json, json};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_render::{Console, escape};
use wg_types::WgError;
use wg_validate::pydantic::error_text;
use wg_validate::{Loc, ValidationError};
use wg_wire::convention::{dict, text};
use wg_wire::error::error_body;

use crate::datetime;
use crate::failure::python_number_text;
use crate::tree::{SNAPSHOT_JSON, placeholder};
use crate::{CliFailure, Snapshot, Usage};

/// The usage of `task list`, `task ready`, `task update`, `task claim` and `task progress`, as
/// Click prints them in a usage error.
const LIST: Usage = Usage {
    usage: "workgraph task list [OPTIONS]",
    command: "workgraph task list",
};
const READY: Usage = Usage {
    usage: "workgraph task ready [OPTIONS]",
    command: "workgraph task ready",
};
const UPDATE: Usage = Usage {
    usage: "workgraph task update [OPTIONS] {ref}",
    command: "workgraph task update",
};
const CLAIM: Usage = Usage {
    usage: "workgraph task claim [OPTIONS] {ref}",
    command: "workgraph task claim",
};
const PROGRESS: Usage = Usage {
    usage: "workgraph task progress [OPTIONS] {ref} {message}",
    command: "workgraph task progress",
};

/// The clap command of the `task` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "task"),
        Err(_) => crate::tree::command("task"),
    }
}

/// Reads a file named by an `@file` option: the text, or the reason it could not be read.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Result<String, String>;

pub fn one(m: &ArgMatches, id: &str) -> Option<String> {
    m.try_get_one::<String>(id).ok().flatten().cloned()
}

pub fn many(m: &ArgMatches, id: &str) -> Vec<String> {
    m.try_get_many::<String>(id)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

pub fn flag(m: &ArgMatches, id: &str) -> bool {
    m.try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

/// The `int` option `id`, else `default`. Click's `int` check: a value Python's `int()` cannot read
/// is a usage error (`at` names the command it is reported under).
pub fn number(m: &ArgMatches, id: &str, default: i64, at: Usage) -> Result<i64, CliFailure> {
    match one(m, id) {
        None => Ok(default),
        Some(text) => python_number_text(&text)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| at.invalid_value(id, &text, "int")),
    }
}

/// `_tag_values`: each value may hold comma-separated tags; blanks are dropped. `None` when no
/// option was given.
fn tag_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

/// `TaskFileIn` limits: a path of 1 to 1000 characters and an anchor of at most 300.
const FILE_PATH_MAX_CHARS: usize = 1000;
const FILE_ANCHOR_MAX_CHARS: usize = 300;

/// `parse_task_file`: `PATH[:ANCHOR]`, where a drive letter belongs to the path. The path may be
/// empty here: [`file_spec_errors`] says whether the pair is a valid `TaskFileIn`.
fn parse_task_file(spec: &str) -> (String, Option<String>) {
    let trimmed = spec.trim();
    let mut chars = trimmed.chars();
    let drive = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':');
    let (head, rest) = if drive {
        trimmed.split_at(2)
    } else {
        ("", trimmed)
    };
    let (path, anchor) = match rest.split_once(':') {
        Some((path, anchor)) => (path, Some(anchor.to_owned())),
        None => (rest, None),
    };
    (format!("{head}{path}"), anchor)
}

/// The pydantic errors of a `TaskFileIn` with this path and anchor, in field order.
fn file_spec_errors(path: &str, anchor: Option<&str>) -> Vec<ValidationError> {
    let path_error = match path.chars().count() {
        0 => Some(length_error(
            "path",
            path,
            "string_too_short",
            "at least 1 character",
        )),
        n if n > FILE_PATH_MAX_CHARS => Some(length_error(
            "path",
            path,
            "string_too_long",
            "at most 1000 characters",
        )),
        _ => None,
    };
    let anchor_error = anchor
        .filter(|anchor| anchor.chars().count() > FILE_ANCHOR_MAX_CHARS)
        .map(|anchor| {
            length_error(
                "anchor",
                anchor,
                "string_too_long",
                "at most 300 characters",
            )
        });
    path_error.into_iter().chain(anchor_error).collect()
}

/// One pydantic string-length error on `field`.
fn length_error(field: &str, input: &str, kind: &str, bound: &str) -> ValidationError {
    ValidationError {
        kind: kind.to_owned(),
        loc: vec![Loc::Key(field.to_owned())],
        msg: format!("String should have {bound}"),
        input: Json::String(input.to_owned()),
        ctx: None,
    }
}

/// `_file_specs`: the `--file` options as request files, refusing the whole command on a bad one.
/// The failure carries pydantic's text for the `TaskFileIn` that did not validate.
fn file_specs(specs: &[String]) -> Result<Vec<Json>, CliFailure> {
    specs
        .iter()
        .map(|spec| {
            let (path, anchor) = parse_task_file(spec);
            let errors = file_spec_errors(&path, anchor.as_deref());
            if errors.is_empty() {
                return Ok(json!({"path": path, "anchor": anchor}));
            }
            Err(CliFailure::Fail {
                message: format!(
                    "invalid --file {}: a path is required before any ':ANCHOR'",
                    py_repr_str(spec)
                ),
                code: 1,
                details: Map::from_iter([(
                    "error".to_owned(),
                    Json::String(error_text("TaskFileIn", &errors)),
                )]),
            })
        })
        .collect()
}

fn put(body: &mut Map<String, Json>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        body.insert(key.to_owned(), Json::String(value));
    }
}

fn put_list(body: &mut Map<String, Json>, key: &str, values: Vec<String>) {
    if !values.is_empty() {
        body.insert(key.to_owned(), json!(values));
    }
}

/// The `--due` option: Click's `datetime` check, which is a usage error (exit 2) on a value none
/// of the formats parses. The value Python sends is the naive datetime's isoformat.
fn due_text(value: &str) -> Result<String, CliFailure> {
    datetime::parse_click_datetime(value)
        .map(|due| wg_json::isoformat(&due))
        .map_err(|why| CliFailure::Usage {
            usage: "workgraph task create [OPTIONS] {title}".to_owned(),
            command: "workgraph task create".to_owned(),
            message: format!("Invalid value for '--due': {why}"),
        })
}

/// The `TaskCreate` body of `task create`.
pub fn create_body(m: &ArgMatches, read_file: ReadFile<'_>) -> Result<Json, CliFailure> {
    // Click checks the options before the command runs, so `--due` fails before `@file` reads.
    let due = one(m, "--due").map(|value| due_text(&value)).transpose()?;
    let mut description = one(m, "--description").unwrap_or_default();
    if let Some(path) = description.strip_prefix('@') {
        description = read_file(path).map_err(|why| CliFailure::fail(why, 1))?;
    }
    let mut body = Map::new();
    body.insert(
        "title".to_owned(),
        json!(one(m, "title").unwrap_or_default()),
    );
    body.insert("description".to_owned(), json!(description));
    body.insert(
        "acceptance_criteria".to_owned(),
        json!(many(m, "--criterion")),
    );
    body.insert(
        "priority".to_owned(),
        json!(one(m, "--priority").unwrap_or_else(|| "medium".to_owned())),
    );
    body.insert(
        "status".to_owned(),
        json!(if flag(m, "--ready") {
            "ready"
        } else {
            "backlog"
        }),
    );
    body.insert("tags".to_owned(), json!(tag_values(&many(m, "--tag"))));
    body.insert(
        "files".to_owned(),
        Json::Array(file_specs(&many(m, "--file"))?),
    );
    body.insert("projects".to_owned(), json!(many(m, "--project")));
    body.insert("repositories".to_owned(), json!(many(m, "--repo")));
    put(&mut body, "parent", one(m, "--parent"));
    body.insert("depends_on".to_owned(), json!(many(m, "--depends-on")));
    put(&mut body, "assignee", one(m, "--assignee"));
    put(&mut body, "assigned_agent", one(m, "--agent"));
    body.insert("auto_execute".to_owned(), json!(flag(m, "--auto-execute")));
    put(&mut body, "due_at", due);
    Ok(Json::Object(body))
}

/// The `TaskFilter` body of `task list`.
pub fn list_body(m: &ArgMatches) -> Result<Json, CliFailure> {
    let mut body = Map::new();
    put_list(&mut body, "statuses", many(m, "--status"));
    put_list(&mut body, "priorities", many(m, "--priority"));
    put(&mut body, "project", one(m, "--project"));
    put(&mut body, "repository", one(m, "--repo"));
    put_list(&mut body, "tags", many(m, "--tag"));
    put(&mut body, "file", one(m, "--file"));
    put(&mut body, "category", one(m, "--category"));
    put(&mut body, "assignee", one(m, "--assignee"));
    put(&mut body, "parent", one(m, "--parent"));
    body.insert("root_only".to_owned(), json!(flag(m, "--roots")));
    put(&mut body, "text", one(m, "--search"));
    body.insert("archived_only".to_owned(), json!(flag(m, "--archived")));
    body.insert(
        "sort".to_owned(),
        json!(one(m, "--sort").unwrap_or_else(|| "created_at".to_owned())),
    );
    body.insert("descending".to_owned(), json!(!flag(m, "--asc")));
    body.insert("limit".to_owned(), json!(number(m, "--limit", 50, LIST)?));
    body.insert("offset".to_owned(), json!(number(m, "--offset", 0, LIST)?));
    Ok(Json::Object(body))
}

/// `parse_json_arg`: JSON text, or `@file` holding it.
fn parse_json_arg(value: &str, what: &str, read_file: ReadFile<'_>) -> Result<Json, CliFailure> {
    let source = match value.strip_prefix('@') {
        Some(path) => read_file(path).map_err(|why| CliFailure::fail(why, 1))?,
        None => value.to_owned(),
    };
    // `json.loads` as CPython words its `JSONDecodeError`, so the message matches the Python CLI.
    wg_validate::pyjson::parse_message(&source)
        .map_err(|why| CliFailure::fail(format!("{what} is not valid JSON: {why}"), 2))
}

/// The `TaskUpdate` body of `task update`.
pub fn update_body(m: &ArgMatches, read_file: ReadFile<'_>) -> Result<Json, CliFailure> {
    let clear_files = flag(m, "--clear-files");
    let files = many(m, "--file");
    if clear_files && !files.is_empty() {
        return Err(CliFailure::fail(
            "--clear-files cannot be combined with --file",
            1,
        ));
    }
    let mut body = Map::new();
    put(&mut body, "title", one(m, "--title"));
    put(&mut body, "description", one(m, "--description"));
    put(&mut body, "status", one(m, "--status"));
    put(&mut body, "priority", one(m, "--priority"));
    put_list(&mut body, "add_tags", tag_values(&many(m, "--add-tag")));
    put_list(
        &mut body,
        "remove_tags",
        tag_values(&many(m, "--remove-tag")),
    );
    if clear_files {
        body.insert("files".to_owned(), json!([]));
    } else if !files.is_empty() {
        body.insert("files".to_owned(), Json::Array(file_specs(&files)?));
    }
    let added = file_specs(&many(m, "--add-file"))?;
    if !added.is_empty() {
        body.insert("add_files".to_owned(), Json::Array(added));
    }
    put(&mut body, "parent", one(m, "--parent"));
    if flag(m, "--clear-parent") {
        body.insert("clear_parent".to_owned(), json!(true));
    }
    put(&mut body, "assignee", one(m, "--assignee"));
    put(&mut body, "assigned_agent", one(m, "--agent"));
    // `v is not False` drops `--no-auto-execute`, as the Python command does.
    if flag(m, "--auto-execute") {
        body.insert("auto_execute".to_owned(), json!(true));
    }
    if let Some(raw) = one(m, "--metadata") {
        let parsed = parse_json_arg(&raw, "metadata", read_file)?;
        if !parsed.is_null() {
            body.insert("metadata".to_owned(), parsed);
        }
    }
    if one(m, "--expected-version").is_some() {
        body.insert(
            "expected_version".to_owned(),
            json!(number(m, "--expected-version", 0, UPDATE)?),
        );
    }
    put(&mut body, "reason", one(m, "--reason"));
    Ok(Json::Object(body))
}

/// The `BulkUpdate` body of `task bulk-update`.
#[must_use]
pub fn bulk_body(m: &ArgMatches) -> Json {
    let mut update = Map::new();
    put(&mut update, "status", one(m, "--status"));
    put(&mut update, "priority", one(m, "--priority"));
    put_list(&mut update, "add_tags", tag_values(&many(m, "--add-tag")));
    put_list(
        &mut update,
        "remove_tags",
        tag_values(&many(m, "--remove-tag")),
    );
    json!({"tasks": many(m, "refs"), "update": update})
}

/// The values of `task promote-ready`: the tags and project, after the scope rules.
pub fn promote_scope(m: &ArgMatches) -> Result<(Vec<String>, Option<String>, bool), CliFailure> {
    let tags = tag_values(&many(m, "--tag"));
    let project = one(m, "--project").filter(|p| !p.is_empty());
    let all = flag(m, "--all");
    if all && (project.is_some() || !tags.is_empty()) {
        return Err(CliFailure::Domain(WgError::new(
            wg_types::ErrorCode::ValidationFailed,
            "--all cannot be combined with --project or --tag",
        )));
    }
    if !(all || project.is_some() || !tags.is_empty()) {
        return Err(CliFailure::Domain(WgError::new(
            wg_types::ErrorCode::ValidationFailed,
            "give --project, --tag or --all to choose the tasks to promote",
        )));
    }
    Ok((tags, project, flag(m, "--dry-run")))
}

/// The `--limit` of `task ready`.
pub fn ready_limit(m: &ArgMatches) -> Result<i64, CliFailure> {
    number(m, "--limit", 20, READY)
}

/// The option values `task ready`, `graph` and `history` read.
#[must_use]
pub fn option_text(m: &ArgMatches, id: &str) -> Option<String> {
    one(m, id).filter(|text| !text.is_empty())
}

/// The values of a repeatable option.
#[must_use]
pub fn option_list(m: &ArgMatches, id: &str) -> Vec<String> {
    many(m, id)
}

/// A flag option.
#[must_use]
pub fn option_flag(m: &ArgMatches, id: &str) -> bool {
    flag(m, id)
}

// ---------------------------------------------------------------- views

pub fn get<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, found)| found),
        Value::Dict(entries) => entries
            .iter()
            .find(|(key, _)| python(&Value::Str(key.clone())) == name)
            .map(|(_, found)| found),
        _ => None,
    }
}

pub fn items(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

/// Python `str()` of a scalar output value.
pub fn python(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => number.to_string(),
        Value::Str(text) => text
            .code_points()
            .iter()
            .filter_map(|point| char::from_u32(*point))
            .collect(),
        Value::DateTime(at) => at.format_display(),
        other => repr(other),
    }
}

/// Python `repr()` of an output value, as `str()` shows the members of a list or dict.
fn repr(value: &Value) -> String {
    match value {
        Value::Str(_) => py_repr_str(&python(value)),
        Value::List(members) => {
            let inner: Vec<String> = members.iter().map(repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Dict(entries) => {
            let inner: Vec<String> = entries
                .iter()
                .map(|(key, member)| {
                    format!("{}: {}", repr(&Value::Str(key.clone())), repr(member))
                })
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::Model { fields, .. } => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(key, member)| format!("{}: {}", py_repr_str(key), repr(member)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        other => python(other),
    }
}

pub fn s(value: Option<&Value>) -> String {
    value.map(python).unwrap_or_default()
}

pub fn opt(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(other) => Some(python(other)),
    }
}

/// `when()`: `YYYY-MM-DD HH:MM`, or nothing.
pub fn when(value: Option<&Value>) -> String {
    match value {
        Some(Value::DateTime(at)) => at.format_display(),
        _ => String::new(),
    }
}

fn count(value: Option<&Value>) -> Option<String> {
    let length = items(value).len();
    (length > 0).then(|| length.to_string())
}

pub fn truncate(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

const HEAD: [&str; 7] = [
    "key",
    "status",
    "priority",
    "category",
    "title",
    "tags",
    "claimed by",
];

fn task_row(task: &Value) -> Vec<String> {
    let tags: Vec<String> = items(get(task, "tags")).iter().map(python).collect();
    vec![
        s(get(task, "key")),
        s(get(task, "status")),
        s(get(task, "priority")),
        opt(get(task, "category")).unwrap_or_default(),
        truncate(&s(get(task, "title")), 70),
        tags.join(","),
        opt(get(task, "claimed_by")).unwrap_or_default(),
    ]
}

/// `emit(out, ...)` of `task create`.
#[must_use]
pub fn created_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "created [bold]{}[/bold] ({}) {}",
            s(get(task, "key")),
            s(get(task, "status")),
            s(get(task, "title"))
        ))
        .text
}

/// The human view of `task list`.
#[must_use]
pub fn list_view(console: &Console, page: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(page, "items")).iter().map(task_row).collect();
    let mut out = console.table(&rows, &HEAD, None).text;
    let next =
        opt(get(page, "next_offset")).map_or_else(String::new, |n| format!("; next: --offset {n}"));
    out.push_str(
        &console
            .print(&format!(
                "[dim]{} of {}{next}[/dim]",
                rows.len(),
                s(get(page, "total"))
            ))
            .text,
    );
    out
}

/// The human view of `task ready`.
#[must_use]
pub fn ready_view(console: &Console, tasks: &Value) -> String {
    let rows: Vec<Vec<String>> = match tasks {
        Value::List(list) => list.iter().map(task_row).collect(),
        _ => Vec::new(),
    };
    console.table(&rows, &HEAD, Some("ready")).text
}

/// `_render_task`.
#[must_use]
pub fn task_view(console: &Console, task: &Value) -> String {
    let tags: Vec<String> = items(get(task, "tags")).iter().map(python).collect();
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("id", opt(get(task, "id"))),
        ("title", opt(get(task, "title"))),
        ("status", opt(get(task, "status"))),
        ("priority", opt(get(task, "priority"))),
        ("category", opt(get(task, "category"))),
        ("tags", Some(tags.join(", "))),
        ("assignee", opt(get(task, "assignee"))),
        ("claimed by", opt(get(task, "claimed_by"))),
        ("parent", opt(get(task, "parent_id"))),
        ("children", count(get(task, "child_ids"))),
        ("depends on", count(get(task, "depends_on"))),
        ("blocks", count(get(task, "blocks"))),
        ("projects", count(get(task, "project_ids"))),
        ("repositories", count(get(task, "repository_ids"))),
        ("files", count(get(task, "files"))),
        ("version", opt(get(task, "version"))),
        ("created", Some(when(get(task, "created_at")))),
        ("completed", Some(when(get(task, "completed_at")))),
        ("accepted", Some(when(get(task, "accepted_at")))),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    let key = s(get(task, "key"));
    let mut out = console.kv(&borrowed, Some(&key)).text;
    let description = s(get(task, "description"));
    if !description.is_empty() {
        out.push_str(&console.print("").text);
        out.push_str(&console.print(&description).text);
    }
    let criteria = items(get(task, "acceptance_criteria"));
    if !criteria.is_empty() {
        out.push_str(&console.print("\n[bold]Acceptance criteria[/bold]").text);
        for item in criteria {
            out.push_str(&console.print(&format!("  - {}", python(item))).text);
        }
    }
    let files = items(get(task, "files"));
    if !files.is_empty() {
        out.push_str(&console.print("\n[bold]Files[/bold]").text);
        for file in files {
            let path = s(get(file, "path"));
            let label = match opt(get(file, "anchor")).filter(|a| !a.is_empty()) {
                Some(anchor) => format!("{path}:{anchor}"),
                None => path,
            };
            let note = opt(get(file, "note"))
                .filter(|n| !n.is_empty())
                .map_or_else(String::new, |n| format!(" - {}", escape(&n)));
            out.push_str(&console.print(&format!("  - {}{note}", escape(&label))).text);
        }
    }
    if let Some(rollup) = get(task, "rollup").filter(|r| !matches!(r, Value::Null)) {
        let accepted = s(get(rollup, "accepted"));
        let total = s(get(rollup, "total_descendants"));
        let progress = match get(rollup, "progress") {
            Some(Value::Float(p)) => format!("{:.0}%", p * 100.0),
            _ => String::new(),
        };
        let by_status = match get(rollup, "by_status") {
            Some(Value::Dict(entries)) => {
                let inner: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}: {}",
                            py_repr_str(&python(&Value::Str(k.clone()))),
                            python(v)
                        )
                    })
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            _ => String::new(),
        };
        out.push_str(
            &console
                .print(&format!(
                    "\n[bold]Rollup[/bold]: {accepted}/{total} accepted ({progress}) {by_status}"
                ))
                .text,
        );
    }
    if let Some(state) = get(task, "dependency_state")
        && matches!(get(state, "satisfied"), Some(Value::Bool(false)))
    {
        let keys: Vec<String> = items(get(state, "unsatisfied"))
            .iter()
            .map(|b| s(get(b, "key")))
            .collect();
        out.push_str(
            &console
                .print(&format!("[yellow]Blocked by:[/yellow] {}", keys.join(", ")))
                .text,
        );
    }
    let comments = items(get(task, "comments"));
    for comment in &comments[comments.len().saturating_sub(10)..] {
        out.push_str(
            &console
                .print(&format!(
                    "\n[dim]{} {} ({})[/dim]\n{}",
                    when(get(comment, "created_at")),
                    s(get(comment, "author")),
                    s(get(comment, "kind")),
                    s(get(comment, "body"))
                ))
                .text,
        );
    }
    out
}

/// The `{items, errors}` value of `task show` with several references.
#[must_use]
pub fn shown_value(found: Vec<Value>, errors: &[(String, WgError)]) -> Value {
    dict(vec![
        ("items", Value::List(found)),
        (
            "errors",
            Value::List(
                errors
                    .iter()
                    .map(|(reference, error)| {
                        dict(vec![
                            ("ref", text(reference)),
                            (
                                "error",
                                error_body(error.code.as_str(), &error.message, &error.details),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// The human view of `task show` with several references: standard output and standard error.
#[must_use]
pub fn shown_view(
    out: &Console,
    err: &Console,
    found: &[Value],
    errors: &[(String, WgError)],
) -> (String, String) {
    let mut stdout = String::new();
    for (index, task) in found.iter().enumerate() {
        if index > 0 {
            stdout.push_str(&out.print("").text);
        }
        stdout.push_str(&task_view(out, task));
    }
    let stderr = errors
        .iter()
        .map(|(reference, error)| {
            err.print(&format!(
                "[red]error[/red] ({}) {}: {}",
                error.code.as_str(),
                escape(reference),
                escape(&error.message)
            ))
            .text
        })
        .collect();
    (stdout, stderr)
}

/// The human view of `task update`.
#[must_use]
pub fn updated_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} updated (v{}, {})",
            s(get(task, "key")),
            s(get(task, "version")),
            s(get(task, "status"))
        ))
        .text
}

/// The human view of `task bulk-update`.
#[must_use]
pub fn bulk_view(console: &Console, result: &Value) -> String {
    let updated = items(get(result, "updated")).len();
    let (failed, shown) = match get(result, "failed") {
        Some(Value::Dict(entries)) if !entries.is_empty() => {
            let inner: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}: {}",
                        py_repr_str(&python(&Value::Str(k.clone()))),
                        py_repr_str(&python(v))
                    )
                })
                .collect();
            (entries.len(), format!("{{{}}}", inner.join(", ")))
        }
        _ => (0, String::new()),
    };
    console
        .print(&format!(
            "updated {updated}; failed {failed} {}",
            escape(&shown)
        ))
        .text
}

/// Whether a bulk result has failures, which make the command exit 3.
#[must_use]
pub fn bulk_failed(result: &Value) -> bool {
    matches!(get(result, "failed"), Some(Value::Dict(entries)) if !entries.is_empty())
}

/// The human view of `task archive`.
#[must_use]
pub fn archived_line(console: &Console, task: &Value, undo: bool) -> String {
    console
        .print(&format!(
            "{} {}",
            s(get(task, "key")),
            if undo { "unarchived" } else { "archived" }
        ))
        .text
}

// ------------------------------------------- claim, release, complete, reports, links

/// The `ClaimRequest` of `task claim`.
pub fn claim_body(m: &ArgMatches) -> Result<Json, CliFailure> {
    Ok(json!({
        "agent": one(m, "--agent"),
        "lease_minutes": number(m, "--lease-minutes", 120, CLAIM)?,
    }))
}

/// The optional `ReasonBody` of `task release`: no body without `--reason`.
#[must_use]
pub fn release_body(m: &ArgMatches) -> Json {
    match one(m, "--reason") {
        None => Json::Null,
        Some(reason) => json!({ "reason": reason }),
    }
}

/// The `CompleteBody` of `task complete`.
#[must_use]
pub fn complete_body(m: &ArgMatches) -> Json {
    json!({
        "summary": one(m, "--summary").unwrap_or_default(),
        "attempt_id": one(m, "--attempt"),
    })
}

/// The `ResultReport` of `task result`; an outcome other than `succeeded` or `failed` is the
/// CLI's own usage error (exit 2), raised before anything is read or written.
pub fn result_body(m: &ArgMatches) -> Result<Json, CliFailure> {
    let outcome = one(m, "outcome").unwrap_or_default();
    if outcome != "succeeded" && outcome != "failed" {
        return Err(CliFailure::fail("outcome must be succeeded or failed", 2));
    }
    Ok(json!({
        "outcome": outcome,
        "summary": one(m, "--summary").unwrap_or_default(),
        "attempt_id": one(m, "--attempt"),
    }))
}

/// The `ProgressReport` of `task progress`; `--percent` is a float, as typer reads it.
pub fn progress_body(m: &ArgMatches) -> Result<Json, CliFailure> {
    let percent = match one(m, "--percent") {
        None => Json::Null,
        Some(text) => {
            let parsed: f64 = python_number_text(&text)
                .and_then(|digits| digits.parse().ok())
                .ok_or_else(|| PROGRESS.invalid_value("--percent", &text, "float"))?;
            json!(parsed)
        }
    };
    Ok(json!({
        "message": one(m, "message").unwrap_or_default(),
        "percent": percent,
        "attempt_id": one(m, "--attempt"),
    }))
}

/// The `CommentCreate` of `task comment`.
#[must_use]
pub fn comment_body(m: &ArgMatches) -> Json {
    json!({ "body": one(m, "body").unwrap_or_default() })
}

/// The `DependencyBody` of `task depend`.
#[must_use]
pub fn dependency_body(depends_on: &str) -> Json {
    json!({ "depends_on": depends_on })
}

/// The `RelationCreate` of `task relate`.
#[must_use]
pub fn relation_body(m: &ArgMatches) -> Json {
    json!({
        "target": one(m, "target").unwrap_or_default(),
        "relation_type": one(m, "--type").unwrap_or_else(|| "relates_to".to_owned()),
    })
}

/// The `RepoBody` of `task link-repo`.
#[must_use]
pub fn repo_body(m: &ArgMatches) -> Json {
    json!({
        "repository": one(m, "repository").unwrap_or_default(),
        "access": one(m, "--access"),
    })
}

/// The human view of `task claim`.
#[must_use]
pub fn claimed_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "claimed {} until {}",
            s(get(task, "key")),
            when(get(task, "claim_expires_at"))
        ))
        .text
}

/// The human view of `task release`.
#[must_use]
pub fn released_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!("released {}", s(get(task, "key"))))
        .text
}

/// The flags of `task accept`: manual, the reason, and whether to promote the dependents.
#[must_use]
pub fn accept_flags(m: &ArgMatches) -> (bool, String, bool) {
    (
        option_flag(m, "--manual"),
        one(m, "--reason").unwrap_or_default(),
        option_flag(m, "--promote-dependents"),
    )
}

/// The reason of `task reject` (required by the command).
#[must_use]
pub fn reject_reason(m: &ArgMatches) -> String {
    one(m, "--reason").unwrap_or_default()
}

/// The human view of `task accept`; with `--promote-dependents` the output also names the
/// dependents it moved to ready.
#[must_use]
pub fn accepted_line(console: &Console, out: &Value, promote: bool) -> String {
    let base = format!(
        "accepted ({}) by {}",
        s(get(out, "mode")),
        s(get(out, "actor"))
    );
    let line = if promote {
        let promoted: Vec<String> = items(get(out, "promoted"))
            .iter()
            .map(|key| s(Some(key)))
            .collect();
        let names = if promoted.is_empty() {
            "no dependents".to_owned()
        } else {
            promoted.join(", ")
        };
        format!("{base}; promoted {names}")
    } else {
        base
    };
    console.print(&line).text
}

/// The human view of `task reject`.
#[must_use]
pub fn rejected_line(console: &Console) -> String {
    console.print("rejected").text
}

/// The human view of `task complete` and `task result`.
#[must_use]
pub fn status_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} is now {}",
            s(get(task, "key")),
            s(get(task, "status"))
        ))
        .text
}

/// A fixed line of the human view (`comment added`, `progress recorded`).
#[must_use]
pub fn note_line(console: &Console, text: &str) -> String {
    console.print(text).text
}

/// The human view of `task depend`.
#[must_use]
pub fn depend_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} depends on {} task(s); status {}",
            s(get(task, "key")),
            items(get(task, "depends_on")).len(),
            s(get(task, "status"))
        ))
        .text
}

/// The human view of `task relate`.
#[must_use]
pub fn related_line(console: &Console, relation: &Value) -> String {
    console
        .print(&format!("related ({})", s(get(relation, "relation_type"))))
        .text
}

/// The human view of `task link-repo`.
#[must_use]
pub fn linked_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} touches {} repositories",
            s(get(task, "key")),
            items(get(task, "repository_ids")).len()
        ))
        .text
}

fn summary(data: Option<&Value>) -> String {
    let Some(data) = data else {
        return String::new();
    };
    if let (Some(from), Some(to)) = (get(data, "from"), get(data, "to")) {
        let reason = opt(get(data, "reason")).filter(|r| !r.is_empty());
        return format!(
            "{} -> {}{}",
            python(from),
            python(to),
            reason.map_or_else(String::new, |r| format!(" ({r})"))
        );
    }
    if let Some(Value::Dict(changes)) = get(data, "changes") {
        let names: Vec<String> = changes
            .iter()
            .map(|(k, _)| python(&Value::Str(k.clone())))
            .collect();
        return names.join(", ");
    }
    if let Value::Dict(entries) = data {
        let joined: Vec<String> = entries
            .iter()
            .take(3)
            .map(|(k, v)| format!("{}={}", python(&Value::Str(k.clone())), python(v)))
            .collect();
        return truncate(&joined.join(", "), 80);
    }
    String::new()
}

/// The human view of `task history`.
#[must_use]
pub fn history_view(console: &Console, events: &Value) -> String {
    let rows: Vec<Vec<String>> = match events {
        Value::List(list) => list
            .iter()
            .map(|event| {
                vec![
                    s(get(event, "seq")),
                    when(get(event, "occurred_at")),
                    s(get(event, "event_type")),
                    s(get(event, "actor")),
                    summary(get(event, "data")),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(&rows, &["seq", "when", "event", "actor", "data"], None)
        .text
}

/// The human view of `task graph`.
#[must_use]
pub fn graph_view(console: &Console, graph: &Value) -> String {
    let nodes = items(get(graph, "nodes"));
    let keys: Vec<(String, String)> = nodes
        .iter()
        .map(|n| (s(get(n, "id")), s(get(n, "key"))))
        .collect();
    let key_of = |id: &str| {
        keys.iter()
            .find(|(candidate, _)| candidate == id)
            .map(|(_, key)| key.clone())
            .unwrap_or_default()
    };
    let node_rows: Vec<Vec<String>> = nodes
        .iter()
        .map(|n| {
            vec![
                s(get(n, "key")),
                s(get(n, "status")),
                if matches!(get(n, "blocked"), Some(Value::Bool(true))) {
                    "BLOCKED".to_owned()
                } else {
                    String::new()
                },
                truncate(&s(get(n, "title")), 60),
            ]
        })
        .collect();
    let edge_rows: Vec<Vec<String>> = items(get(graph, "edges"))
        .iter()
        .map(|e| {
            let kind = s(get(e, "kind"));
            vec![
                key_of(&s(get(e, "source"))),
                if kind == "depends_on" {
                    "blocks".to_owned()
                } else {
                    kind
                },
                key_of(&s(get(e, "target"))),
            ]
        })
        .collect();
    let mut out = console
        .table(&node_rows, &["key", "status", "", "title"], None)
        .text;
    out.push_str(
        &console
            .table(&edge_rows, &["from", "edge", "to"], None)
            .text,
    );
    out
}

/// The human view of `task promote-ready`.
#[must_use]
pub fn promote_view(console: &Console, report: &Value, dry_run: bool) -> String {
    let keys: Vec<String> = items(get(report, "promoted")).iter().map(python).collect();
    let mut line = format!(
        "{} {}",
        if dry_run { "would promote" } else { "promoted" },
        keys.len()
    );
    if !keys.is_empty() {
        line.push_str(&format!(": {}", keys.join(", ")));
    }
    let mut out = console.print(&line).text;
    if let Some(Value::Dict(counts)) = get(report, "by_project") {
        for (slug, number) in counts {
            out.push_str(
                &console
                    .print(&format!(
                        "  {}: {}",
                        escape(&python(&Value::Str(slug.clone()))),
                        python(number)
                    ))
                    .text,
            );
        }
    }
    out
}
