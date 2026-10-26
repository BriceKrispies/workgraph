//! Shell: the acceptance a tracker sync records for a done issue (`record_external_acceptance`).
//!
//! The caller is inside its write transaction: it passes the session it writes through, the stamp
//! of its decision and the task. This loads the task's latest succeeded attempt and returns the
//! decision of [`crate::spec::external`]. The caller applies it and moves the task to `accepted`.

use wg_model::Decision;
use wg_model::entity::Task;
use wg_model::stamp::Stamp;
use wg_runtime::latest_succeeded;
use wg_store::Session;
use wg_types::{Id, WgError};

use crate::spec::external;

/// The acceptance that tracker connection `connection` implies for `task`. The reply is the
/// `task.accepted` event id.
pub fn record_external_acceptance(
    session: &mut Session<'_>,
    stamp: &Stamp,
    task: &Task,
    connection: &str,
) -> Result<Decision<Id>, WgError> {
    let attempt = latest_succeeded(session, task.id)?;
    Ok(external::record_external_acceptance(
        stamp,
        task,
        attempt.as_ref(),
        connection,
    ))
}
