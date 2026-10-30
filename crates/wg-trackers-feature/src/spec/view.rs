//! The human views of the `tracker` commands (`cli/commands/tracker.py`): the table of connections,
//! the status block of each connection, and the line an enqueue prints.

use wg_iface::cli::args::python;
use wg_render::Console;
use wg_trackers::SyncReport;
use wg_wire::convention as wire;

use super::{Connection, Status, iso_of_millis};

/// Python's `str()` of a list of strings: its repr, as `kv` prints a list value.
fn list_text(items: &[String]) -> String {
    python(&wire::texts(items.iter().map(String::as_str)))
}

/// The repr of a list for `kv`, left out when the list is empty (Python's `value in ([], {})`).
fn list_value(items: &[String]) -> Option<String> {
    (!items.is_empty()).then(|| list_text(items))
}

/// The human view of `tracker list`: one row per connection.
#[must_use]
pub fn list_view(console: &Console, rows: &[Connection]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.name.clone(),
                row.flavour.clone(),
                row.base_url.clone(),
                row.workspace.clone(),
                row.projects.join(","),
            ]
        })
        .collect();
    console
        .table(
            &cells,
            &["name", "flavour", "base_url", "workspace", "projects"],
            None,
        )
        .text
}

/// The human view of `tracker status`: a block per status, in the order given.
#[must_use]
pub fn status_view(console: &Console, statuses: &[Status]) -> String {
    statuses
        .iter()
        .map(|status| {
            let tracked = status.tracked_tasks.to_string();
            let missing = status.missing.to_string();
            let cursor = status.cursor.and_then(iso_of_millis);
            let last_sync = status.last_sync.and_then(iso_of_millis);
            let last_full_sync = status.last_full_sync.and_then(iso_of_millis);
            let projects = list_value(&status.projects);
            let unresolved = list_value(&status.unresolved_pull_request_repos);
            let pairs: [(&str, Option<&str>); 9] = [
                ("connection", Some(status.connection.as_str())),
                ("workspace", Some(status.workspace.as_str())),
                ("projects", projects.as_deref()),
                ("cursor", cursor.as_deref()),
                ("last_sync", last_sync.as_deref()),
                ("last_full_sync", last_full_sync.as_deref()),
                ("tracked_tasks", Some(tracked.as_str())),
                ("missing", Some(missing.as_str())),
                ("unresolved_pull_request_repos", unresolved.as_deref()),
            ];
            let title = format!("tracker {}", status.connection);
            console.kv(&pairs, Some(title.trim())).text
        })
        .collect()
}

/// The line `tracker sync --queue` prints.
#[must_use]
pub fn enqueued_line(console: &Console, job_id: &str) -> String {
    console.print(&format!("enqueued {job_id}")).text
}

/// The human view of `tracker sync` (`kv` under the title `tracker sync`).
#[must_use]
pub fn sync_view(console: &Console, report: &SyncReport) -> String {
    let rows = crate::spec::run::sync_rows(report);
    let pairs: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|(name, value)| (*name, value.as_deref()))
        .collect();
    console.kv(&pairs, Some("tracker sync")).text
}
