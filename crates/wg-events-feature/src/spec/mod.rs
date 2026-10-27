//! The pure parts of the `events` commands (`cli/commands/data.py`): the clap command, the readers
//! of its matches, the history query, the `--json` documents and the human views. Nothing here
//! reads the store; [`crate::service`] runs the reads and [`crate::cli`] prints.

use clap::{ArgMatches, Command};
use serde_json::Value as Json;
use wg_clispec::args::{cell, get, items, one, when};
use wg_clispec::tree::snapshot_command;
use wg_clispec::{CliFailure, Usage};
use wg_json::Value;
use wg_render::Console;
use wg_storecodec::EventQuery;
use wg_types::pyval::prefix;
use wg_types::{Id, parse_id};

/// The most events one history read returns (`min(limit, 5000)`).
pub const HISTORY_LIMIT_CAP: i64 = 5000;

/// The `events list` usage line, for the `--limit` and `--after` errors.
const LIST: Usage = Usage {
    usage: "workgraph events list [OPTIONS]",
    command: "workgraph events list",
};

/// The clap command of the `events` group, from the snapshot so help and parameters match
/// `contracts/cli.json`.
#[must_use]
pub fn command() -> Command {
    snapshot_command("events")
}

/// The options of `events list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListArgs {
    /// `--entity`: the entity id to filter by.
    pub entity: Option<String>,
    /// `--prefix`: the event type prefix.
    pub prefix: Option<String>,
    /// `--after`: only events after this sequence number.
    pub after: Option<i64>,
    /// `--limit`, 50 by default.
    pub limit: i64,
}

/// Reads the options of `events list`.
///
/// # Errors
///
/// Returns a usage error when `--after` or `--limit` is not an integer.
pub fn list_args(m: &ArgMatches) -> Result<ListArgs, CliFailure> {
    let after = match one(m, "--after") {
        None => None,
        Some(_) => Some(wg_clispec::args::number(m, "--after", 0, LIST)?),
    };
    Ok(ListArgs {
        entity: one(m, "--entity"),
        prefix: one(m, "--prefix"),
        after,
        limit: wg_clispec::args::number(m, "--limit", 50, LIST)?,
    })
}

/// `-w/--workspace` of the `events` commands, when given.
#[must_use]
pub fn workspace_arg(m: &ArgMatches) -> Option<String> {
    one(m, "--workspace")
}

/// The history query of `events list` (`history.events`): the workspace when one is named, the
/// entity, the prefix and the sequence cursor. Newest first unless `--after` is given.
///
/// `None` when `--entity` is not an id: no event carries it, so the answer is empty. An empty
/// `--entity` is no filter, as in Python (`if entity_id`).
#[must_use]
pub fn list_query(args: &ListArgs, workspace: Option<Id>) -> Option<EventQuery> {
    let entity_ids = match args.entity.as_deref().filter(|text| !text.is_empty()) {
        Some(text) => vec![parse_id(text).ok()?],
        None => Vec::new(),
    };
    Some(EventQuery {
        workspace_id: workspace,
        entity_ids,
        event_type_prefix: args.prefix.clone(),
        after_seq: args.after,
        limit: args.limit.min(HISTORY_LIMIT_CAP),
        descending: args.after.is_none(),
        ..EventQuery::default()
    })
}

/// The `--json` document of `events verify`: the report of `verify_task_projection`, as the
/// object the Python command prints.
#[must_use]
pub fn verify_document(report: &Json) -> Value {
    from_json(report)
}

/// The human view of `events list`: one row per event.
#[must_use]
pub fn list_view(console: &Console, events: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(events))
        .iter()
        .map(|event| {
            vec![
                cell(get(event, "seq")),
                when(get(event, "occurred_at")),
                cell(get(event, "event_type")),
                cell(get(event, "actor")),
                prefix(&cell(get(event, "entity_id")), 8),
            ]
        })
        .collect();
    console
        .table(&rows, &["seq", "when", "event", "actor", "entity"], None)
        .text
}

/// The human line of `events verify`: `consistent` or `MISMATCH`, the counts, and the mismatches
/// (as Python's `str()` of the dict) when there are any.
#[must_use]
pub fn verify_view(console: &Console, report: &Value) -> String {
    let consistent = matches!(get(report, "consistent"), Some(Value::Bool(true)));
    let tail = if consistent {
        String::new()
    } else {
        format!(" {}", cell(get(report, "mismatches")))
    };
    console
        .print(&format!(
            "{}: {} tasks, {} events{}",
            if consistent { "consistent" } else { "MISMATCH" },
            cell(get(report, "tasks")),
            cell(get(report, "events")),
            tail
        ))
        .text
}

/// A JSON document, as the output value of the same Python object: object keys in file order,
/// integers as integers.
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
        Json::String(string) => Value::Str(wg_json::PyStr::from(string.as_str())),
        Json::Array(members) => Value::List(members.iter().map(from_json).collect()),
        Json::Object(map) => Value::Dict(
            map.iter()
                .map(|(key, member)| (wg_json::PyStr::from(key.as_str()), from_json(member)))
                .collect(),
        ),
    }
}
