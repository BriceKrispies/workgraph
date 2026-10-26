//! wg-jobs: queue semantics, leases, worker liveness, maintenance and retention (GV-20, CORE-19).
//!
//! Mirrors `src/workgraph/jobs/` (`queue.py`, `limits.py`, `trackers.py`, `maintenance.py`,
//! `retention.py`). Every rule is a pure function over a `QueueSlice` (or a plain input) that
//! returns a `Decision`: the row writes the shell applies in one transaction. Time comes only
//! from `Stamp::at`; ids come only from `Stamp::id`. The worker loop, handler dispatch and the
//! file removal of retention are shell glue and live in the runtime crates.

use wg_types::Timestamp;

pub mod dispatch;
pub mod limits;
pub mod maintenance;
pub mod queue;
pub mod retention;
pub mod scheduler;
pub mod trackers;
pub mod workers;

/// `at` moved by `seconds` (negative values move it back). Sub-microsecond parts are rounded
/// and an out-of-range result keeps `at`, so the function never panics.
pub fn shift(at: Timestamp, seconds: f64) -> Timestamp {
    let delta = time::Duration::microseconds((seconds * 1_000_000.0).round() as i64);
    at.to_offset_datetime()
        .checked_add(delta)
        .and_then(|moved| Timestamp::from_offset_datetime(moved).ok())
        .unwrap_or(at)
}

/// The first `limit` characters of `text` (Python's `text[:limit]` counts code points).
pub(crate) fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
