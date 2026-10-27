//! The telemetry feature's maintenance step (`HookIngestor.replay_spool` as the maintenance leader
//! runs it in `jobs/maintenance.py`). The worker collects [`MAINTENANCE`] through the feature's
//! `FEATURE` value.

use serde_json::Value as Json;
use wg_jobs::maintenance::Step;
use wg_runtime::{App, MaintenanceStep, RequestContext};
use wg_types::WgError;

use crate::service::claude;

/// The most spooled hook records one maintenance pass replays (`replay_spool(limit=200)`).
const REPLAY_LIMIT: i64 = 200;

/// The maintenance steps this feature fills: the hook spool replay.
pub static MAINTENANCE: &[MaintenanceStep] = &[MaintenanceStep {
    slot: Step::ReplayHooks,
    run: replay_hooks,
}];

/// The spool replay: the `ingested`, `duplicate` and `failed` counts of the records it handled.
fn replay_hooks(app: &mut App, _context: &RequestContext) -> Result<Json, WgError> {
    claude::replay(app, REPLAY_LIMIT)
}
