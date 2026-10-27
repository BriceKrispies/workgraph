//! Pure: the settings of one tracker connection as a sync reads them, and the documents of a sync
//! report (`tracker sync` in `cli/commands/tracker.py`, and the job result of `tracker_sync`).

use serde_json::{Map, Value as Json};
use wg_json::{Value, dict};
use wg_netspec::TrackerError;
use wg_trackers::SyncReport;
use wg_types::WgError;
use wg_wire::convention as wire;

use wg_clispec::args::python;

use super::{iso_of_millis, unknown_connection};
use wg_types::pyval::{int_or, text};

/// A transport failure of a listing, as the sync raises it.
#[must_use]
pub fn tracker_failure(err: TrackerError) -> WgError {
    WgError::workgraph(err.message)
}

/// One `[trackers.<name>]` connection, as `TrackerSyncService` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackerSettings {
    pub name: String,
    pub base_url: String,
    /// `cloud` or `data_center`.
    pub flavour: String,
    pub workspace: String,
    pub projects: Vec<String>,
    pub email_env: Option<String>,
    pub token_env: String,
    pub overlap_seconds: i64,
    pub missing_grace_seconds: i64,
    pub dev_panel: bool,
    pub pr_fields: Vec<String>,
    pub cancelled_resolutions: Vec<String>,
    pub timeout_seconds: f64,
}

/// `TrackerSyncService.connection`: the settings of the named connection, or the `not_found` the
/// Python service raises.
pub fn settings_of(config: &Map<String, Json>, name: &str) -> Result<TrackerSettings, WgError> {
    let tracker = config
        .get("trackers")
        .and_then(Json::as_object)
        .and_then(|trackers| trackers.get(name))
        .ok_or_else(|| unknown_connection(name))?;
    let text = |key: &str| text(tracker.get(key)).to_owned();
    let texts = |key: &str| -> Vec<String> {
        tracker
            .get(key)
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let int = |key: &str, default: i64| int_or(tracker.get(key), default);
    let flavour = match tracker.get("flavour").and_then(Json::as_str) {
        Some(flavour) => flavour.to_owned(),
        None => "cloud".to_owned(),
    };
    Ok(TrackerSettings {
        name: name.to_owned(),
        base_url: text("base_url"),
        flavour,
        workspace: text("workspace"),
        projects: texts("projects"),
        email_env: tracker
            .get("email_env")
            .and_then(Json::as_str)
            .map(str::to_owned),
        token_env: text("token_env"),
        overlap_seconds: int("overlap_seconds", 120),
        missing_grace_seconds: int("missing_grace_seconds", 86_400),
        dev_panel: tracker
            .get("dev_panel")
            .and_then(Json::as_bool)
            .unwrap_or(true),
        pr_fields: texts("pr_fields"),
        cancelled_resolutions: texts("cancelled_resolutions"),
        timeout_seconds: tracker
            .get("timeout_seconds")
            .and_then(Json::as_f64)
            .unwrap_or(30.0),
    })
}

/// The report as `tracker sync --json` prints it (`SyncReport.as_dict`), keys in Python's order.
#[must_use]
pub fn sync_out(report: &SyncReport) -> Value {
    dict!({
        "connection": wire::text(&report.connection),
        "full": Value::Bool(report.full),
        "fetched": wire::int(report.fetched),
        "created": wire::int(report.created),
        "updated": wire::int(report.updated),
        "unchanged": wire::int(report.unchanged),
        "missing": wire::int(report.missing),
        "archived": wire::int(report.archived),
        "unresolved_pull_requests": wire::texts(report.unresolved_pull_requests.iter().map(String::as_str)),
        "errors": wire::texts(report.errors.iter().map(String::as_str)),
        "cursor": cursor_value(report.cursor),
    })
}

/// The report as the `tracker_sync` job stores it: the same keys, as a JSON object.
#[must_use]
pub fn sync_json(report: &SyncReport) -> Map<String, Json> {
    let cursor = report
        .cursor
        .and_then(iso_of_millis)
        .map_or(Json::Null, Json::String);
    let texts = |items: &[String]| Json::Array(items.iter().cloned().map(Json::String).collect());
    [
        ("connection", Json::String(report.connection.clone())),
        ("full", Json::Bool(report.full)),
        ("fetched", Json::from(report.fetched)),
        ("created", Json::from(report.created)),
        ("updated", Json::from(report.updated)),
        ("unchanged", Json::from(report.unchanged)),
        ("missing", Json::from(report.missing)),
        ("archived", Json::from(report.archived)),
        (
            "unresolved_pull_requests",
            texts(&report.unresolved_pull_requests),
        ),
        ("errors", texts(&report.errors)),
        ("cursor", cursor),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// The human view's rows of a report, as `kv` prints them (empty lists are left out).
#[must_use]
pub fn sync_rows(report: &SyncReport) -> Vec<(&'static str, Option<String>)> {
    let cursor = report.cursor.and_then(iso_of_millis);
    vec![
        ("connection", Some(report.connection.clone())),
        (
            "full",
            Some(if report.full { "True" } else { "False" }.to_owned()),
        ),
        ("fetched", Some(report.fetched.to_string())),
        ("created", Some(report.created.to_string())),
        ("updated", Some(report.updated.to_string())),
        ("unchanged", Some(report.unchanged.to_string())),
        ("missing", Some(report.missing.to_string())),
        ("archived", Some(report.archived.to_string())),
        (
            "unresolved pull requests",
            list_value(&report.unresolved_pull_requests),
        ),
        ("errors", list_value(&report.errors)),
        ("cursor", cursor),
    ]
}

/// The repr of a list for `kv`, left out when the list is empty (Python's `value in ([], {})`).
fn list_value(items: &[String]) -> Option<String> {
    (!items.is_empty()).then(|| python(&wire::texts(items.iter().map(String::as_str))))
}

fn cursor_value(cursor: Option<i64>) -> Value {
    wire::optional_text(cursor.and_then(iso_of_millis).as_deref())
}
