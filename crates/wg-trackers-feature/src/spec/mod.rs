//! Pure: the statement text and row decoding of the external item family, and the tracker
//! commands' documents (`cli/commands/tracker.py`): the connection list, the status and the enqueue
//! reply. The clap command is in [`args`], the human views in [`view`].

pub mod args;
pub mod credentials;
pub mod run;
pub mod sql;
pub mod sync;
pub mod view;

use serde_json::{Map, Value as Json};
use time::OffsetDateTime;
use wg_json::{Value, dict};
use wg_model::py_repr_str;
use wg_types::pyval::text;
use wg_types::{Timestamp, WgError};
use wg_wire::convention as wire;

/// One configured tracker connection (Python `JiraTrackerConfig`), as `tracker list` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub name: String,
    pub flavour: String,
    pub base_url: String,
    pub workspace: String,
    pub projects: Vec<String>,
}

/// What `tracker status` reports for one connection (`TrackerSyncService.status`). The cursors are
/// epoch milliseconds, as `system_cursors` stores them.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub connection: String,
    pub workspace: String,
    pub projects: Vec<String>,
    pub cursor: Option<i64>,
    pub last_sync: Option<i64>,
    pub last_full_sync: Option<i64>,
    pub tracked_tasks: i64,
    pub missing: i64,
    pub unresolved_pull_request_repos: Vec<String>,
}

/// `_cursor_name(connection, suffix)`: the name of a sync cursor in `system_cursors`.
#[must_use]
pub fn cursor_name(connection: &str, suffix: &str) -> String {
    format!("tracker:{connection}:{suffix}")
}

/// The connections of the configuration `config`, in name order (`sorted(config.trackers.items())`).
#[must_use]
pub fn connections(config: &Map<String, Json>) -> Vec<Connection> {
    let mut rows: Vec<Connection> = config
        .get("trackers")
        .and_then(Json::as_object)
        .map(|trackers| {
            trackers
                .iter()
                .map(|(name, tracker)| Connection {
                    name: name.clone(),
                    flavour: text_field(tracker, "flavour"),
                    base_url: text_field(tracker, "base_url"),
                    workspace: text_field(tracker, "workspace"),
                    projects: tracker
                        .get("projects")
                        .and_then(Json::as_array)
                        .map(|keys| {
                            keys.iter()
                                .filter_map(Json::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

/// A text field of a configuration table, empty when it is missing.
fn text_field(tracker: &Json, key: &str) -> String {
    text(tracker.get(key)).to_owned()
}

/// The configured connection `name`, or the `not_found` the Python service raises
/// (`TrackerSyncService.connection`).
pub fn connection_of(connections: &[Connection], name: &str) -> Result<Connection, WgError> {
    connections
        .iter()
        .find(|connection| connection.name == name)
        .cloned()
        .ok_or_else(|| unknown_connection(name))
}

/// `TrackerSyncService.connection`'s refusal: the connection is not configured (`not_found`).
#[must_use]
pub fn unknown_connection(name: &str) -> WgError {
    WgError::not_found(format!(
        "tracker connection {} is not configured",
        py_repr_str(name)
    ))
    .detail("connection", Json::String(name.to_owned()))
}

/// `datetime.fromtimestamp(ms / 1000, tz=UTC).isoformat()`: the instant of a cursor in ISO 8601,
/// with a fraction only when it is not whole seconds. `None` when the instant is out of range.
#[must_use]
pub fn iso_of_millis(ms: i64) -> Option<String> {
    let at = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()?;
    Timestamp::from_offset_datetime(at)
        .ok()
        .map(Timestamp::format_isoformat)
}

/// An optional cursor as its ISO 8601 text, or `null`.
fn optional_iso(ms: Option<i64>) -> Value {
    wire::optional_text(ms.and_then(iso_of_millis).as_deref())
}

/// `tracker list --json`: one row per connection.
#[must_use]
pub fn list_out(rows: &[Connection]) -> Value {
    Value::List(
        rows.iter()
            .map(|row| {
                dict!({
                    "name": wire::text(&row.name),
                    "flavour": wire::text(&row.flavour),
                    "base_url": wire::text(&row.base_url),
                    "workspace": wire::text(&row.workspace),
                    "projects": wire::texts(row.projects.iter().map(String::as_str)),
                })
            })
            .collect(),
    )
}

/// One status as its document, in the Python dict's key order.
#[must_use]
pub fn status_out(status: &Status) -> Value {
    dict!({
        "connection": wire::text(&status.connection),
        "workspace": wire::text(&status.workspace),
        "projects": wire::texts(status.projects.iter().map(String::as_str)),
        "cursor": optional_iso(status.cursor),
        "last_sync": optional_iso(status.last_sync),
        "last_full_sync": optional_iso(status.last_full_sync),
        "tracked_tasks": wire::int(status.tracked_tasks),
        "missing": wire::int(status.missing),
        "unresolved_pull_request_repos": wire::texts(
            status
                .unresolved_pull_request_repos
                .iter()
                .map(String::as_str),
        ),
    })
}

/// The `status` document: one dict when a connection was named, else the list of every
/// connection's status.
#[must_use]
pub fn status_document(statuses: &[Status], single: bool) -> Value {
    match statuses.first() {
        Some(first) if single => status_out(first),
        _ => Value::List(statuses.iter().map(status_out).collect()),
    }
}

/// `tracker sync --queue --json`: the job, the connection and the mode.
#[must_use]
pub fn enqueued_out(job_id: &str, connection: &str, full: bool) -> Value {
    dict!({
        "job_id": wire::text(job_id),
        "connection": wire::text(connection),
        "full": Value::Bool(full),
    })
}
