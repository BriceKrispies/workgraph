//! Worker settings and the rules of the worker loop (`jobs/worker.py`, `jobs/handlers.py`): the
//! 17 job kinds, how a handler's failure maps onto a retry decision, the heartbeat cadence and the
//! settings read from the configuration.
//!
//! The threads, the handlers and the I/O of the loop are the runtime's; every rule they apply is
//! a function here.

use serde_json::{Map, Value};
use wg_types::ErrorCode;

use crate::limits::LimitSettings;
use crate::maintenance::TrackerCadence;
use crate::retention::RetentionPolicy;

/// Every job kind the Python `HANDLERS` table maps, in its order.
pub const JOB_KINDS: [&str; 17] = [
    "git_sync",
    "tracker_sync",
    "analyze_commits",
    "compute_change_surface",
    "classify_task",
    "embed_task",
    "evaluate_classifier",
    "agent_run",
    "verify",
    "stack_integrate",
    "stack_verify",
    "decompose_task",
    "automation_action",
    "generate_report",
    "failure_analysis",
    "compact_telemetry",
    "noop",
];

/// The kinds of [`JOB_KINDS`] absent from `wired`, in the table's order.
pub fn unwired_kinds(wired: &[String]) -> Vec<&'static str> {
    JOB_KINDS
        .into_iter()
        .filter(|kind| !wired.iter().any(|name| name == kind))
        .collect()
}

/// Seconds between polls of an idle worker.
pub const DEFAULT_POLL_SECONDS: f64 = 1.0;

/// Seconds between maintenance passes.
pub const DEFAULT_MAINTENANCE_SECONDS: f64 = 5.0;

/// Seconds running jobs get on shutdown.
pub const DEFAULT_GRACE_SECONDS: f64 = 30.0;

/// The error kind recorded for a job whose kind has no registered handler.
pub const ERROR_KIND_NOT_PORTED: &str = "handler_not_ported";

/// The error kind recorded for a handler that panicked (Python's `exception`).
pub const ERROR_KIND_EXCEPTION: &str = "exception";

/// Seconds without a worker heartbeat after which maintenance marks a worker lost.
pub const STALE_WORKER_SECONDS: i64 = 120;

/// How a failed job is recorded: whether the queue may retry it and the error kind kept on the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub retryable: bool,
    pub error_kind: String,
}

/// The retry decision for a handler error of class `code`: validation, not-found and permission
/// errors never succeed on a retry; every other class (provider outages included) is retried.
pub fn failure_for(code: ErrorCode) -> Failure {
    let retryable = !matches!(
        code,
        ErrorCode::ValidationFailed | ErrorCode::NotFound | ErrorCode::PermissionDenied
    );
    Failure {
        retryable,
        error_kind: code.as_str().to_owned(),
    }
}

/// The failure of a job whose kind has no handler, and its message: never retried.
pub fn not_ported(kind: &str) -> (Failure, String) {
    (
        Failure {
            retryable: false,
            error_kind: ERROR_KIND_NOT_PORTED.to_owned(),
        },
        format!("handler not ported for job kind '{kind}'"),
    )
}

/// Seconds between a running job's heartbeats: a third of the lease, at least one second.
pub fn heartbeat_interval_seconds(lease_seconds: i64) -> f64 {
    (lease_seconds as f64 / 3.0).max(1.0)
}

/// The lease the maintenance leader takes: three poll intervals, at least ten seconds.
pub fn maintenance_lease_seconds(maintenance_interval_seconds: f64) -> i64 {
    ((maintenance_interval_seconds * 3.0) as i64).max(10)
}

/// Whether a job that has run for `elapsed_seconds` has outlived its `timeout_seconds`.
pub fn timed_out(timeout_seconds: Option<f64>, elapsed_seconds: f64) -> bool {
    timeout_seconds.is_some_and(|limit| limit != 0.0 && elapsed_seconds > limit)
}

/// `{host}-{pid}-{6 hex digits}` (the default worker id).
pub fn default_worker_id(host: &str, pid: u32, entropy: &[u8]) -> String {
    let suffix: String = entropy
        .iter()
        .take(3)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{host}-{pid}-{suffix}")
}

/// What the worker and maintenance read from the effective configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerSettings {
    pub concurrency: i64,
    pub lease_seconds: i64,
    pub limits: LimitSettings,
    /// `execution.enabled and execution.auto_schedule`.
    pub scheduler_enabled: bool,
    pub retention: RetentionPolicy,
    pub trackers: Vec<TrackerCadence>,
}

fn section<'a>(config: &'a Map<String, Value>, name: &str) -> Option<&'a Map<String, Value>> {
    config.get(name).and_then(Value::as_object)
}

fn int(section: Option<&Map<String, Value>>, key: &str, default: i64) -> i64 {
    section
        .and_then(|map| map.get(key))
        .and_then(Value::as_i64)
        .unwrap_or(default)
}

fn flag(section: Option<&Map<String, Value>>, key: &str) -> bool {
    section
        .and_then(|map| map.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

impl WorkerSettings {
    /// Reads the settings from a configuration with its defaults filled in; a missing member
    /// takes its default.
    pub fn from_config(config: &Map<String, Value>) -> WorkerSettings {
        let jobs = section(config, "jobs");
        let execution = section(config, "execution");
        let retention = section(config, "retention");
        let trackers = section(config, "trackers")
            .map(|map| {
                map.iter()
                    .map(|(name, connection)| {
                        let connection = connection.as_object();
                        TrackerCadence {
                            connection: name.clone(),
                            poll_interval_seconds: int(connection, "poll_interval_seconds", 300),
                            full_sync_interval_seconds: int(
                                connection,
                                "full_sync_interval_seconds",
                                86_400,
                            ),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        WorkerSettings {
            concurrency: int(jobs, "worker_concurrency", 4),
            lease_seconds: int(jobs, "lease_seconds", 60),
            limits: LimitSettings {
                gpu_concurrency: int(jobs, "gpu_concurrency", 1),
                per_agent_concurrency: int(jobs, "per_agent_concurrency", 2),
                per_repository_concurrency: int(jobs, "per_repository_concurrency", 1),
                verify_concurrency: int(jobs, "verify_concurrency", 1),
                max_concurrent_runs: int(execution, "max_concurrent_runs", 2),
            },
            scheduler_enabled: flag(execution, "enabled") && flag(execution, "auto_schedule"),
            retention: RetentionPolicy {
                telemetry_payload_days: int(retention, "telemetry_payload_days", 30),
                run_log_days: int(retention, "run_log_days", 90),
            },
            trackers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_errors_that_cannot_succeed_are_not_retried() {
        assert!(!failure_for(ErrorCode::ValidationFailed).retryable);
        assert!(!failure_for(ErrorCode::NotFound).retryable);
        assert!(!failure_for(ErrorCode::PermissionDenied).retryable);
        assert!(failure_for(ErrorCode::ProviderUnavailable).retryable);
        assert!(failure_for(ErrorCode::GitError).retryable);
        assert_eq!(failure_for(ErrorCode::Conflict).error_kind, "conflict");
    }

    #[test]
    fn cadences_follow_the_python_worker() {
        assert_eq!(heartbeat_interval_seconds(60), 20.0);
        assert_eq!(heartbeat_interval_seconds(2), 1.0);
        assert_eq!(maintenance_lease_seconds(5.0), 15);
        assert_eq!(maintenance_lease_seconds(1.0), 10);
        assert!(timed_out(Some(1.0), 1.5));
        assert!(!timed_out(Some(0.0), 99.0));
        assert!(!timed_out(None, 99.0));
    }

    #[test]
    fn the_unwired_kinds_are_the_table_minus_the_wired_ones() {
        let wired = vec!["noop".to_owned()];
        let unwired = unwired_kinds(&wired);
        assert_eq!(unwired.len(), JOB_KINDS.len() - 1);
        assert!(!unwired.contains(&"noop"));
    }

    #[test]
    fn an_empty_config_takes_the_defaults() {
        let settings = WorkerSettings::from_config(&Map::new());
        assert_eq!(settings.concurrency, 4);
        assert_eq!(settings.lease_seconds, 60);
        assert_eq!(settings.limits.max_concurrent_runs, 2);
        assert!(!settings.scheduler_enabled);
        assert_eq!(settings.retention.run_log_days, 90);
        assert!(settings.trackers.is_empty());
    }
}
