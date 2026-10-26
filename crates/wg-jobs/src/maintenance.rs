//! Maintenance leadership, cadence and plan (`jobs/maintenance.py`).
//!
//! One worker at a time holds the `system:maintenance` lease and runs the periodic steps. Each
//! periodic step is gated by a cursor row (`maintenance:<name>`) that records when it last ran.

use wg_model::decision::Decision;
use wg_model::entity::{ResourceLease, SystemCursor};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::write::Write;

use crate::shift;

/// The resource key whose lease elects the maintenance leader.
pub const LEADER_LEASE_KEY: &str = "system:maintenance";

/// The purpose recorded on the leader lease.
pub const LEADER_PURPOSE: &str = "maintenance";

/// Retention runs at most once per this many seconds (24 hours).
pub const RETENTION_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// Elects `holder` as maintenance leader (`_elect`, through `acquire_lease`).
///
/// `existing` is the current lease row for [`LEADER_LEASE_KEY`]. The reply is `true` when the
/// holder leads afterwards. A lease held by another worker is busy until its expiry; the holder
/// itself renews its lease, and an expired lease is taken over.
pub fn elect(
    existing: Option<&ResourceLease>,
    holder: &str,
    ttl_seconds: i64,
    stamp: &Stamp,
) -> Decision<bool> {
    let now = stamp.at;
    let expires_at = Some(shift(now, ttl_seconds as f64));
    let fresh = ResourceLease {
        resource_key: LEADER_LEASE_KEY.to_owned(),
        holder: holder.to_owned(),
        purpose: LEADER_PURPOSE.to_owned(),
        acquired_at: now,
        expires_at,
    };
    match existing {
        None => Decision::new(true).write(Write::insert(fresh)),
        Some(lease) if lease.holder == holder => {
            Decision::new(true).write(Write::update(ResourceLease {
                expires_at,
                ..lease.clone()
            }))
        }
        Some(lease) if lease_is_busy(lease, now) => Decision::new(false),
        Some(lease) => Decision::new(true)
            .write(Write::Delete(lease.row_key()))
            .write(Write::insert(fresh)),
    }
}

/// Whether another holder's lease still blocks election: it has no expiry, or expires after `now`.
fn lease_is_busy(lease: &ResourceLease, now: wg_types::Timestamp) -> bool {
    lease.expires_at.is_none_or(|expires| expires > now)
}

/// Whether the periodic step `name` is due, and if so records that it runs now (`_due`).
///
/// A step with no cursor is due at once and gets a cursor. Otherwise it is due when at least
/// `interval_seconds` have passed since the cursor value, measured in whole seconds.
pub fn due(slice: &QueueSlice, name: &str, interval_seconds: i64, stamp: &Stamp) -> Decision<bool> {
    let key = format!("maintenance:{name}");
    let now = stamp.at;
    let seconds = now.to_offset_datetime().unix_timestamp();
    match slice.cursors.get(&key) {
        None => Decision::new(true).write(Write::insert(SystemCursor {
            name: key,
            value: seconds,
            updated_at: now,
        })),
        Some(cursor) if seconds - cursor.value >= interval_seconds => {
            Decision::new(true).write(Write::update(SystemCursor {
                value: seconds,
                updated_at: now,
                ..cursor.clone()
            }))
        }
        Some(_) => Decision::new(false),
    }
}

/// The periodic steps of one maintenance pass, in the order the Python pass runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    RecoverExpired,
    MarkLostWorkers,
    ReleaseExpiredClaims,
    ProcessAutomationEvents,
    RunScheduledRules,
    ScheduledRuns,
    ReplayHooks,
    TrackerSyncs,
    Retention,
}

/// The steps a pass runs. `ScheduledRuns` runs only when the scheduler is enabled and auto
/// scheduling is on (`execution.enabled and execution.auto_schedule`).
pub fn plan(scheduler_enabled: bool) -> Vec<Step> {
    [
        Step::RecoverExpired,
        Step::MarkLostWorkers,
        Step::ReleaseExpiredClaims,
        Step::ProcessAutomationEvents,
        Step::RunScheduledRules,
        Step::ScheduledRuns,
        Step::ReplayHooks,
        Step::TrackerSyncs,
        Step::Retention,
    ]
    .into_iter()
    .filter(|step| *step != Step::ScheduledRuns || scheduler_enabled)
    .collect()
}

/// A configured tracker connection with its two sync intervals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerCadence {
    pub connection: String,
    pub poll_interval_seconds: i64,
    pub full_sync_interval_seconds: i64,
}

/// A sync the pass should queue: the connection and whether it is a full sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerSyncDue {
    pub connection: String,
    pub full: bool,
}

/// Which tracker syncs are due, in configuration order, poll before full for each connection.
/// The cursors advance in the returned writes; the shell queues each returned sync
/// (`_enqueue_tracker_syncs`).
pub fn tracker_syncs_due(
    slice: &QueueSlice,
    connections: &[TrackerCadence],
    stamp: &Stamp,
) -> Decision<Vec<TrackerSyncDue>> {
    let checks: Vec<(Option<TrackerSyncDue>, Vec<Write>)> = connections
        .iter()
        .flat_map(|cadence| {
            [
                (cadence, false, "poll", cadence.poll_interval_seconds),
                (cadence, true, "full", cadence.full_sync_interval_seconds),
            ]
        })
        .map(|(cadence, full, mode, interval)| {
            let check = due(
                slice,
                &format!("tracker:{}:{mode}", cadence.connection),
                interval,
                stamp,
            );
            let sync = check.reply.then(|| TrackerSyncDue {
                connection: cadence.connection.clone(),
                full,
            });
            (sync, check.writes.iter().cloned().collect())
        })
        .collect();
    let (syncs, writes): (Vec<Option<TrackerSyncDue>>, Vec<Vec<Write>>) =
        checks.into_iter().unzip();
    writes.into_iter().flatten().fold(
        Decision::new(syncs.into_iter().flatten().collect()),
        Decision::write,
    )
}
