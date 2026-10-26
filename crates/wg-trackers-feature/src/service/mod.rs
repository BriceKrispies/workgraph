//! Shell: the tracker commands' entry points (`tracker list, status, sync`, C168 to C170) and the
//! external item readers. Free functions over `&mut App`, the load-decide-apply glue; the decisions
//! are in [`crate::spec`] and the sync decisions in the `wg-trackers` core crate.

pub mod reads;

use wg_jobs::trackers::{TRACKER_SYNC_KIND, enqueue_tracker_sync};
use wg_runtime::{App, RequestContext};
use wg_store::Session;
use wg_types::{Id, WgError};

use crate::spec::{self, Connection, Status};

/// `tracker list`: the configured connections, in name order.
pub fn list(app: &mut App) -> Result<Vec<Connection>, WgError> {
    Ok(spec::connections(&app.config()?))
}

/// `tracker status [connection]`: one status for the named connection, else one per connection.
pub fn status(app: &mut App, connection: Option<&str>) -> Result<Vec<Status>, WgError> {
    let connections = spec::connections(&app.config()?);
    let selected = match connection {
        Some(name) => vec![spec::connection_of(&connections, name)?],
        None => connections,
    };
    selected
        .iter()
        .map(|connection| read_status(app, connection))
        .collect()
}

/// The value of the sync cursor `suffix` of `connection` (`_read_cursor`), in epoch milliseconds.
fn cursor_value(
    session: &mut Session<'_>,
    connection: &str,
    suffix: &str,
) -> Result<Option<i64>, WgError> {
    Ok(session
        .system_cursor(&spec::cursor_name(connection, suffix))?
        .map(|row| row.value))
}

/// The cursors, tracked counts and unresolved pull request repositories of one connection.
fn read_status(app: &mut App, connection: &Connection) -> Result<Status, WgError> {
    let name = connection.name.as_str();
    app.runtime.query(
        |session| {
            Ok(Status {
                connection: name.to_owned(),
                workspace: connection.workspace.clone(),
                projects: connection.projects.clone(),
                cursor: cursor_value(session, name, "updated")?,
                last_sync: cursor_value(session, name, "last_sync")?,
                last_full_sync: cursor_value(session, name, "last_full_sync")?,
                tracked_tasks: reads::tracked_count(session, name)?,
                missing: reads::missing_count(session, name)?,
                unresolved_pull_request_repos: reads::unresolved_pull_request_repos(session, name)?,
            })
        },
        |status| Ok(status.clone()),
    )
}

/// The `not_found` check every `tracker sync` makes first (`TrackerSyncService.connection`).
pub fn check_connection(app: &mut App, connection: &str) -> Result<(), WgError> {
    let connections = spec::connections(&app.config()?);
    spec::connection_of(&connections, connection).map(|_| ())
}

/// `tracker sync --queue`: queues a sync of `connection` and returns its job id. A queued or
/// running sync of the same connection and mode is returned instead of queued again.
pub fn enqueue_sync(
    app: &mut App,
    context: &RequestContext,
    connection: &str,
    full: bool,
) -> Result<Id, WgError> {
    let names: Vec<String> = spec::connections(&app.config()?)
        .into_iter()
        .map(|connection| connection.name)
        .collect();
    let known: Vec<&str> = names.iter().map(String::as_str).collect();
    let created_by = context.actor.to_string();
    Ok(app
        .runtime
        .decide(
            context,
            |session| session.active_slice(TRACKER_SYNC_KIND),
            |slice, stamp| {
                enqueue_tracker_sync(slice, connection, full, &known, &created_by, stamp)
            },
        )?
        .reply)
}
