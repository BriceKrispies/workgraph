//! Retention and compaction of verbose telemetry (`jobs/retention.py`).
//!
//! Only telemetry artifacts (run and verification logs) are compacted here: a purged artifact keeps
//! its metadata and gains a `purged_at` timestamp. The files themselves are removed by the shell,
//! which passes in the root check (`is_within`) and deletes only the paths the decision selects.
//! Hook payload compaction waits for the knowledge-family entities (LIB-11).

use wg_model::decision::Decision;
use wg_model::entity::Artifact;
use wg_model::stamp::Stamp;
use wg_model::write::Write;
use wg_types::Timestamp;

use crate::shift;

/// The retention policy of `config.retention`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub telemetry_payload_days: i64,
    pub run_log_days: i64,
}

/// What one compaction pass purged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompactReport {
    pub artifacts: usize,
    pub bytes_freed: i64,
}

const SECONDS_PER_DAY: f64 = 86_400.0;

/// The instants before which telemetry payloads and run logs are stale: `(payload, log)`.
pub fn cutoffs(policy: &RetentionPolicy, now: Timestamp) -> (Timestamp, Timestamp) {
    (
        shift(
            now,
            -(policy.telemetry_payload_days as f64) * SECONDS_PER_DAY,
        ),
        shift(now, -(policy.run_log_days as f64) * SECONDS_PER_DAY),
    )
}

/// Marks the telemetry artifacts created before the run-log cutoff as purged.
///
/// `is_within_root` says whether the artifact's path lies inside the artifacts directory; an
/// artifact outside it is never selected. The reply counts the selected artifacts and the bytes
/// they held. The shell removes the selected files and applies the writes (a dry run applies
/// neither).
pub fn compact_artifacts(
    artifacts: &[Artifact],
    policy: &RetentionPolicy,
    stamp: &Stamp,
    is_within_root: impl Fn(&str) -> bool,
) -> Decision<CompactReport> {
    let (_, log_cutoff) = cutoffs(policy, stamp.at);
    let selected: Vec<&Artifact> = artifacts
        .iter()
        .filter(|artifact| {
            artifact.retention == "telemetry"
                && artifact.created_at < log_cutoff
                && artifact.purged_at.is_none()
                && is_within_root(&artifact.path)
        })
        .collect();
    let report = CompactReport {
        artifacts: selected.len(),
        bytes_freed: selected.iter().fold(0, |total, artifact| {
            total.saturating_add(artifact.size_bytes)
        }),
    };
    selected
        .into_iter()
        .map(|artifact| {
            Write::update(Artifact {
                purged_at: Some(stamp.at),
                ..artifact.clone()
            })
        })
        .fold(Decision::new(report), Decision::write)
}
