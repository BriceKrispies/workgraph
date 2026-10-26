//! The `task` command group's clap command and the request bodies its handlers build from the
//! parsed options. The generic readers are in `wg_clispec::args`.

use clap::ArgMatches;
use serde_json::{Map, Value as Json, json};
use wg_clispec::args::{ReadFile, flag, get, many, number, one, option_flag};
use wg_clispec::datetime;
use wg_clispec::failure::python_number_text;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{CliFailure, Snapshot, Usage};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_types::WgError;
use wg_validate::pydantic::error_text;
use wg_validate::{Loc, ValidationError};
use wg_wire::convention::{dict, text};
use wg_wire::error::error_body;

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
        Err(_) => wg_clispec::tree::command("task"),
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

/// Whether a bulk result has failures, which make the command exit 3.
#[must_use]
pub fn bulk_failed(result: &Value) -> bool {
    matches!(get(result, "failed"), Some(Value::Dict(entries)) if !entries.is_empty())
}

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
