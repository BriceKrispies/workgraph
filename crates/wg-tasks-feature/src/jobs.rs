//! The tasks feature's maintenance step (`TaskService.release_expired_claims` as the maintenance
//! leader runs it in `jobs/maintenance.py`). The worker collects [`MAINTENANCE`] through the
//! feature's `FEATURE` value.

use serde_json::{Value as Json, json};
use wg_jobs::maintenance::Step;
use wg_runtime::{App, MaintenanceStep, RequestContext};
use wg_types::WgError;

use crate::service;

/// The maintenance steps this feature fills: the expired-claim sweep.
pub static MAINTENANCE: &[MaintenanceStep] = &[MaintenanceStep {
    slot: Step::ReleaseExpiredClaims,
    run: release_expired_claims,
}];

/// The sweep (`release_expired_claims`): the keys of the tasks whose claim lease ran out.
fn release_expired_claims(app: &mut App, context: &RequestContext) -> Result<Json, WgError> {
    service::release_expired_claims(app, context).map(|keys| json!(keys))
}
