//! Ports of `tests/unit/test_jobs.py` and `tests/unit/test_tracker_jobs.py` onto the pure decisions
//! of `wg_jobs`, plus value tests for leadership, worker liveness and retention.
//!
//! The Python tests that start a worker thread, call a job handler or run a tracker sync are shell
//! behaviour (the worker loop and the handlers are not in this crate) and are not ported here.

use std::collections::BTreeMap;

use rpds::RedBlackTreeSetSync;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use wg_jobs::limits::{LimitSettings, verify_key};
use wg_jobs::maintenance::{self, Step, TrackerCadence, TrackerSyncDue};
use wg_jobs::queue::{self, ClaimedJob, JobFilter, NewJob};
use wg_jobs::retention::{self, RetentionPolicy};
use wg_jobs::trackers::{TRACKER_SYNC_KIND, enqueue_tracker_sync};
use wg_jobs::workers::{self, NewWorker};
use wg_model::entity::{Artifact, JsonObject, ResourceLease, SystemCursor};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, derive_id};
use wg_types::{Actor, ErrorCode, Id, JobStatus, Timestamp, WgError};

const EPOCH_UNIX_SECONDS: i64 = 1_767_225_600;
const CONNECTIONS: [&str; 2] = ["work-jira", "other"];

fn ok<T>(result: Result<T, WgError>) -> T {
    result.unwrap_or_else(|err| panic!("unexpected error: {err:?}"))
}

fn at(seconds: f64) -> Timestamp {
    let epoch = OffsetDateTime::from_unix_timestamp(EPOCH_UNIX_SECONDS)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let moment = epoch + Duration::microseconds((seconds * 1_000_000.0).round() as i64);
    ok(Timestamp::from_offset_datetime(moment))
}

fn limits() -> LimitSettings {
    LimitSettings {
        gpu_concurrency: 1,
        per_agent_concurrency: 1,
        per_repository_concurrency: 1,
        verify_concurrency: 2,
        max_concurrent_runs: 2,
    }
}

/// A slice the tests advance by applying each decision's writes, with a stamp per decision.
struct Db {
    slice: QueueSlice,
    seq: u32,
}

impl Db {
    fn new() -> Self {
        Db {
            slice: QueueSlice::default(),
            seq: 0,
        }
    }

    fn stamp(&mut self, seconds: f64) -> Stamp {
        self.seq += 1;
        let mut entropy = [0u8; 32];
        entropy[..4].copy_from_slice(&self.seq.to_be_bytes());
        Stamp {
            at: at(seconds),
            entropy,
            actor: ok(Actor::parse("system:test")),
            permissions: RedBlackTreeSetSync::new_sync(),
            correlation: derive_id(&entropy, 0).into(),
            causation: None,
            attempt: None,
            automation_depth: 0,
        }
    }

    fn apply<R>(&mut self, decision: Decision<R>) -> R {
        self.slice = self.slice.apply_all(decision.writes.iter());
        decision.reply
    }

    fn apply_result<R>(&mut self, result: Result<Decision<R>, WgError>) -> Result<R, WgError> {
        result.map(|decision| self.apply(decision))
    }

    fn enqueue(&mut self, seconds: f64, job: &NewJob) -> Id {
        let stamp = self.stamp(seconds);
        let decision = queue::enqueue(&self.slice, job, &stamp);
        self.apply(decision)
    }

    fn claim(
        &mut self,
        worker: &str,
        seconds: f64,
        kinds: &[&str],
        lease_seconds: i64,
    ) -> Option<ClaimedJob> {
        let stamp = self.stamp(seconds);
        let decision = queue::claim(&self.slice, worker, kinds, lease_seconds, &limits(), &stamp);
        self.apply(decision)
    }

    fn claim_any(&mut self, worker: &str, seconds: f64) -> Option<ClaimedJob> {
        self.claim(worker, seconds, &[], 60)
    }

    fn complete(&mut self, seconds: f64, id: Id, worker: &str) -> Result<(), WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::complete(&self.slice, id, worker, None, &stamp);
        self.apply_result(decision)
    }

    fn fail(
        &mut self,
        seconds: f64,
        id: Id,
        worker: &str,
        error: &str,
        retryable: bool,
    ) -> Result<JobStatus, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::fail(&self.slice, id, worker, error, retryable, None, &stamp);
        self.apply_result(decision)
    }

    fn heartbeat(&mut self, seconds: f64, id: Id, worker: &str) -> Result<bool, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::heartbeat(&self.slice, id, worker, 60, &stamp);
        self.apply_result(decision)
    }

    fn cancel(&mut self, seconds: f64, id: Id) -> Result<JobStatus, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::cancel(&self.slice, id, &stamp);
        self.apply_result(decision)
    }

    fn recover(&mut self, seconds: f64) -> Vec<Id> {
        let stamp = self.stamp(seconds);
        let decision = queue::recover_expired(&self.slice, &stamp);
        self.apply(decision)
    }

    fn status(&self, id: Id) -> JobStatus {
        ok(queue::get(&self.slice, id)).status
    }

    fn tracker(&mut self, seconds: f64, connection: &str, full: bool) -> Result<Id, WgError> {
        let stamp = self.stamp(seconds);
        let decision = enqueue_tracker_sync(
            &self.slice,
            connection,
            full,
            &CONNECTIONS,
            "human:tester",
            &stamp,
        );
        self.apply_result(decision)
    }

    /// Tracker sync jobs that are queued or running, as `(connection, full, status)`.
    fn tracker_jobs(&self) -> Vec<(String, bool, JobStatus)> {
        let (listed, _) = queue::list(
            &self.slice,
            &JobFilter {
                kind: Some(TRACKER_SYNC_KIND),
                ..JobFilter::default()
            },
        );
        let mut rows: Vec<(String, bool, JobStatus)> = listed
            .into_iter()
            .map(|job| {
                let connection = job
                    .payload
                    .get("connection")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let full = job
                    .payload
                    .get("full")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (connection, full, job.status)
            })
            .collect();
        rows.sort();
        rows
    }
}

fn payload(entries: &[(&str, Value)]) -> JsonObject {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn job(kind: &str) -> NewJob {
    NewJob::new(kind)
}

fn keyed(kind: &str, keys: &[String]) -> NewJob {
    NewJob {
        resource_keys: keys.to_vec(),
        ..NewJob::new(kind)
    }
}

// ---------------------------------------------------------------- test_jobs.py

#[test]
fn enqueue_idempotency_and_claim() {
    let mut db = Db::new();
    let first = db.enqueue(
        0.0,
        &NewJob {
            payload: payload(&[("x", json!(1))]),
            idempotency_key: Some("k1".to_owned()),
            ..job("noop")
        },
    );
    let second = db.enqueue(
        0.0,
        &NewJob {
            payload: payload(&[("x", json!(2))]),
            idempotency_key: Some("k1".to_owned()),
            ..job("noop")
        },
    );
    assert_eq!(first, second);
    let claimed = db
        .claim_any("w1", 0.0)
        .unwrap_or_else(|| panic!("nothing claimed"));
    assert_eq!((claimed.id, claimed.attempt), (first, 1));
    assert!(db.claim_any("w2", 0.0).is_none());
    ok(db.complete(0.0, first, "w1"));
    assert_eq!(db.status(first), JobStatus::Succeeded);
    let again = db.complete(0.0, first, "w1");
    assert_eq!(again.err().map(|err| err.code), Some(ErrorCode::Conflict));
}

#[test]
fn retry_backoff_and_dead_letter() {
    let mut db = Db::new();
    let job_id = db.enqueue(
        0.0,
        &NewJob {
            max_attempts: 2,
            backoff_base_seconds: 10.0,
            ..job("noop")
        },
    );
    let claimed = db.claim_any("w", 0.0);
    assert!(claimed.is_some());
    assert_eq!(
        ok(db.fail(0.0, job_id, "w", "boom", true)),
        JobStatus::Queued
    );
    let run_at = ok(queue::get(&db.slice, job_id)).run_at;
    assert!(run_at > at(9.0));
    assert!(db.claim_any("w", 0.0).is_none(), "backoff not elapsed");
    let claimed = db.claim_any("w", 30.0);
    assert_eq!(claimed.map(|found| found.attempt), Some(2));
    assert_eq!(
        ok(db.fail(30.0, job_id, "w", "boom again", true)),
        JobStatus::Dead
    );
    assert_eq!(db.status(job_id), JobStatus::Dead);
    let stamp = db.stamp(30.0);
    let retried = queue::retry_dead(&db.slice, job_id, &stamp);
    ok(db.apply_result(retried));
    assert_eq!(db.status(job_id), JobStatus::Queued);
    assert_eq!(
        queue::backoff_delay(2.0, 1, queue::BACKOFF_CAP_SECONDS),
        2.0
    );
    assert_eq!(
        queue::backoff_delay(2.0, 4, queue::BACKOFF_CAP_SECONDS),
        16.0
    );
}

#[test]
fn non_retryable_failure_goes_dead_immediately() {
    let mut db = Db::new();
    let job_id = db.enqueue(
        0.0,
        &NewJob {
            max_attempts: 5,
            ..job("noop")
        },
    );
    assert!(db.claim_any("w", 0.0).is_some());
    assert_eq!(
        ok(db.fail(0.0, job_id, "w", "bad input", false)),
        JobStatus::Dead
    );
}

#[test]
fn lease_expiry_recovery() {
    let mut db = Db::new();
    let job_id = db.enqueue(
        0.0,
        &NewJob {
            max_attempts: 3,
            ..job("noop")
        },
    );
    assert!(db.claim("crashed-worker", 0.0, &[], 5).is_some());
    assert!(db.recover(2.0).is_empty());
    assert_eq!(db.recover(10.0), vec![job_id]);
    let row = ok(queue::get(&db.slice, job_id));
    assert_eq!(row.status, JobStatus::Queued);
    assert_eq!(row.error_kind.as_deref(), Some("lease_expired"));
    let renewed = db.heartbeat(10.0, job_id, "crashed-worker");
    assert_eq!(renewed.err().map(|err| err.code), Some(ErrorCode::Conflict));
}

#[test]
fn resource_limits() {
    let mut db = Db::new();
    let gpu = vec!["gpu".to_owned()];
    let first = db.enqueue(0.0, &keyed("noop", &gpu));
    let second = db.enqueue(1.0, &keyed("noop", &gpu));
    let other = db.enqueue(2.0, &keyed("noop", &["repo:abc".to_owned()]));
    let claimed = db.claim_any("w", 3.0).map(|found| found.id);
    assert_eq!(claimed, Some(first));
    let next = db.claim_any("w", 3.0).map(|found| found.id);
    assert_eq!(next, Some(other), "gpu slot busy; independent job proceeds");
    assert!(db.claim_any("w", 3.0).is_none());
    ok(db.complete(3.0, first, "w"));
    let again = db.claim_any("w", 3.0).map(|found| found.id);
    assert_eq!(again, Some(second));
}

#[test]
fn verify_keys_are_bounded_by_verify_concurrency() {
    let mut db = Db::new();
    let verify = vec![verify_key("abc")];
    let jobs: Vec<Id> = (0..3)
        .map(|i| db.enqueue(f64::from(i), &keyed("noop", &verify)))
        .collect();
    let first = db.claim_any("w", 3.0).map(|found| found.id);
    let second = db.claim_any("w", 3.0).map(|found| found.id);
    assert_eq!(
        [first, second].into_iter().flatten().collect::<Vec<Id>>(),
        jobs[..2].to_vec()
    );
    assert!(
        db.claim_any("w", 3.0).is_none(),
        "two verifications hold the verify keys"
    );
    ok(db.complete(3.0, jobs[0], "w"));
    assert_eq!(db.claim_any("w", 3.0).map(|found| found.id), Some(jobs[2]));
}

#[test]
fn cancellation() {
    let mut db = Db::new();
    let queued = db.enqueue(0.0, &job("noop"));
    assert_eq!(ok(db.cancel(0.0, queued)), JobStatus::Cancelled);
    let running = db.enqueue(0.0, &job("noop"));
    assert!(db.claim_any("w", 0.0).is_some());
    assert_eq!(ok(db.cancel(0.0, running)), JobStatus::Running);
    assert!(ok(db.heartbeat(0.0, running, "w")));
}

#[test]
fn concurrent_claims_never_duplicate() {
    // The Python test races six threads; the decision is pure, so the same property is checked by
    // letting six workers take turns until the queue is empty.
    let mut db = Db::new();
    for i in 0..40 {
        db.enqueue(f64::from(i), &payloaded(i));
    }
    let mut claimed: Vec<Id> = Vec::new();
    let workers = ["w0", "w1", "w2", "w3", "w4", "w5"];
    let mut turn = 0usize;
    let mut idle = 0usize;
    while idle < workers.len() {
        let name = workers[turn % workers.len()];
        turn += 1;
        match db.claim_any(name, 100.0) {
            Some(found) => {
                idle = 0;
                claimed.push(found.id);
                ok(db.complete(100.0, found.id, name));
            }
            None => idle += 1,
        }
    }
    let distinct: std::collections::BTreeSet<Id> = claimed.iter().copied().collect();
    assert_eq!(claimed.len(), 40);
    assert_eq!(distinct.len(), 40);
}

fn payloaded(i: i32) -> NewJob {
    NewJob {
        payload: payload(&[("i", json!(i))]),
        ..job("noop")
    }
}

// ---------------------------------------------------------- test_tracker_jobs.py

#[test]
fn enqueue_is_a_no_op_while_a_sync_is_active() {
    let mut db = Db::new();
    let first = ok(db.tracker(0.0, "work-jira", false));
    assert_eq!(ok(db.tracker(0.0, "work-jira", false)), first);
    let running = db.claim_any("w1", 0.0).map(|found| found.id);
    assert_eq!(running, Some(first));
    assert_eq!(
        ok(db.tracker(0.0, "work-jira", false)),
        first,
        "still running"
    );
    let full = ok(db.tracker(0.0, "work-jira", true));
    assert_ne!(
        full, first,
        "a full sync is a different job from an incremental one"
    );
    ok(db.complete(0.0, first, "w1"));
    let later = ok(db.tracker(0.0, "work-jira", false));
    assert!(
        later != first && later != full,
        "finished syncs never block a new one"
    );
}

#[test]
fn enqueue_uses_one_resource_key_per_connection() {
    let mut db = Db::new();
    ok(db.tracker(0.0, "work-jira", false));
    ok(db.tracker(1.0, "other", false));
    let first = db.claim_any("w1", 2.0);
    assert_eq!(
        first.and_then(|found| found
            .payload
            .get("connection")
            .and_then(Value::as_str)
            .map(str::to_owned)),
        Some("work-jira".to_owned())
    );
    assert!(
        db.claim_any("w2", 2.0).is_some(),
        "the other connection is not held back"
    );
    assert!(db.claim_any("w3", 2.0).is_none());
}

#[test]
fn enqueue_rejects_unknown_connection() {
    let mut db = Db::new();
    let rejected = db.tracker(0.0, "nope", false);
    assert_eq!(
        rejected.err().map(|err| err.code),
        Some(ErrorCode::NotFound)
    );
    assert!(db.tracker_jobs().is_empty());
}

fn cadence() -> Vec<TrackerCadence> {
    vec![TrackerCadence {
        connection: "work-jira".to_owned(),
        poll_interval_seconds: 30,
        full_sync_interval_seconds: 300,
    }]
}

/// One maintenance pass over the tracker cadence: due syncs are queued, cursors advance.
fn tracker_pass(db: &mut Db, seconds: f64, connections: &[TrackerCadence]) -> Vec<TrackerSyncDue> {
    let stamp = db.stamp(seconds);
    let due = maintenance::tracker_syncs_due(&db.slice, connections, &stamp);
    let syncs = db.apply(due);
    // Each sync is queued a microsecond after the previous one, as the Python pass does, so the
    // poll sync is created before the full sync and claims order by creation.
    for (index, sync) in syncs.iter().enumerate() {
        let moment = seconds + index as f64 * 0.000_001;
        let _ = ok(db.tracker(moment, &sync.connection, sync.full));
    }
    syncs
}

#[test]
fn maintenance_enqueues_each_sync_once_per_interval() {
    let mut db = Db::new();
    let first = tracker_pass(&mut db, 0.0, &cadence());
    assert_eq!(
        first,
        vec![
            TrackerSyncDue {
                connection: "work-jira".to_owned(),
                full: false
            },
            TrackerSyncDue {
                connection: "work-jira".to_owned(),
                full: true
            },
        ]
    );
    assert_eq!(db.tracker_jobs().len(), 2);

    // Nothing is due again until the poll interval has elapsed.
    assert!(tracker_pass(&mut db, 0.0, &cadence()).is_empty());
    assert_eq!(db.tracker_jobs().len(), 2);

    // Poll interval elapsed, full interval not: one more incremental, a no-op while the earlier
    // incremental is still queued.
    let poll = tracker_pass(&mut db, 31.0, &cadence());
    assert_eq!(
        poll,
        vec![TrackerSyncDue {
            connection: "work-jira".to_owned(),
            full: false
        }]
    );
    assert_eq!(db.tracker_jobs().len(), 2);

    // Once the queued sync has finished, the next poll queues a new one.
    let claimed = db.claim_any("w1", 31.0);
    let claimed_id = claimed
        .map(|found| found.id)
        .unwrap_or_else(|| panic!("nothing claimed"));
    ok(db.complete(31.0, claimed_id, "w1"));
    let poll = tracker_pass(&mut db, 62.0, &cadence());
    assert_eq!(poll.len(), 1);
    assert_eq!(db.tracker_jobs().len(), 3);

    let full = tracker_pass(&mut db, 301.0, &cadence());
    assert!(
        full.iter().any(|sync| sync.full),
        "the full interval has elapsed"
    );
}

#[test]
fn failing_connection_is_listed_in_order_so_the_shell_can_isolate_it() {
    // The Python test makes one enqueue raise and checks that the other steps still run. The raise
    // is shell behaviour; the decision lists every due sync in configuration order (the shell
    // isolates a failing enqueue) and the plan keeps the steps after tracker syncs.
    let mut db = Db::new();
    let connections = vec![
        TrackerCadence {
            connection: "broken".to_owned(),
            poll_interval_seconds: 30,
            full_sync_interval_seconds: 300,
        },
        TrackerCadence {
            connection: "work-jira".to_owned(),
            poll_interval_seconds: 30,
            full_sync_interval_seconds: 300,
        },
    ];
    let stamp = db.stamp(0.0);
    let due = maintenance::tracker_syncs_due(&db.slice, &connections, &stamp);
    let syncs = db.apply(due);
    let names: Vec<(&str, bool)> = syncs
        .iter()
        .map(|sync| (sync.connection.as_str(), sync.full))
        .collect();
    assert_eq!(
        names,
        vec![
            ("broken", false),
            ("broken", true),
            ("work-jira", false),
            ("work-jira", true)
        ]
    );
    let steps = maintenance::plan(false);
    let tracker_at = steps.iter().position(|step| *step == Step::TrackerSyncs);
    assert_eq!(
        tracker_at.map(|index| steps[index + 1..].to_vec()),
        Some(vec![Step::Retention])
    );
}

// ------------------------------------------------ leadership, liveness, retention

fn lease(holder: &str, expires_at: Option<Timestamp>) -> ResourceLease {
    ResourceLease {
        resource_key: maintenance::LEADER_LEASE_KEY.to_owned(),
        holder: holder.to_owned(),
        purpose: maintenance::LEADER_PURPOSE.to_owned(),
        acquired_at: at(0.0),
        expires_at,
    }
}

fn stamp_only(seconds: f64) -> Stamp {
    let mut db = Db::new();
    db.stamp(seconds)
}

#[test]
fn leadership_is_taken_renewed_and_released_by_expiry() {
    let stamp = stamp_only(0.0);
    let free = maintenance::elect(None, "m1", 30, &stamp);
    assert!(free.reply);
    assert_eq!(free.writes.len(), 1);

    let held = lease("m1", Some(at(30.0)));
    let renewed = maintenance::elect(Some(&held), "m1", 30, &stamp_only(10.0));
    assert!(renewed.reply);

    let blocked = maintenance::elect(Some(&held), "m2", 30, &stamp_only(10.0));
    assert!(!blocked.reply);
    assert!(blocked.writes.is_empty());

    let expired = maintenance::elect(Some(&held), "m2", 30, &stamp_only(31.0));
    assert!(expired.reply);
    assert_eq!(expired.writes.len(), 2, "the expired lease is replaced");
    let open_ended = lease("m1", None);
    assert!(!maintenance::elect(Some(&open_ended), "m2", 30, &stamp_only(1e6)).reply);
}

#[test]
fn cursor_gates_each_periodic_step() {
    let interval = maintenance::RETENTION_INTERVAL_SECONDS;
    let mut db = Db::new();
    let first = db.stamp(0.0);
    let decision = maintenance::due(&db.slice, "retention", interval, &first);
    assert!(decision.reply, "no cursor: due at once");
    db.slice = db.slice.apply_all(decision.writes.iter());

    let soon = db.stamp(3600.0);
    assert!(!maintenance::due(&db.slice, "retention", interval, &soon).reply);

    let later = db.stamp(86_400.0);
    let decision = maintenance::due(&db.slice, "retention", interval, &later);
    assert!(decision.reply, "a full interval has elapsed");
    db.slice = db.slice.apply_all(decision.writes.iter());
    let cursor: Option<SystemCursor> = db.slice.cursors.get("maintenance:retention").cloned();
    assert_eq!(
        cursor.map(|found| found.value),
        Some(EPOCH_UNIX_SECONDS + 86_400)
    );
}

#[test]
fn plan_runs_scheduled_runs_only_when_the_scheduler_is_enabled() {
    assert!(!maintenance::plan(false).contains(&Step::ScheduledRuns));
    assert!(maintenance::plan(true).contains(&Step::ScheduledRuns));
    assert_eq!(maintenance::plan(false).last(), Some(&Step::Retention));
}

#[test]
fn stale_workers_are_marked_lost_and_fresh_ones_are_kept() {
    let mut db = Db::new();
    let registered = |id: &str| NewWorker {
        id: id.to_owned(),
        hostname: "host".to_owned(),
        pid: 1,
        concurrency: 2,
        kinds: vec!["noop".to_owned()],
    };
    let stamp = db.stamp(0.0);
    let decision = workers::register_worker(&db.slice, &registered("old"), &stamp);
    db.apply(decision);
    let stamp = db.stamp(100.0);
    let decision = workers::register_worker(&db.slice, &registered("fresh"), &stamp);
    db.apply(decision);
    let stamp = db.stamp(150.0);
    let beat = workers::worker_heartbeat(&db.slice, "fresh", &[], 0, 0, &stamp);
    db.apply(beat);
    let stamp = db.stamp(200.0);
    let decision = workers::mark_lost_workers(&db.slice, 120, &stamp);
    let lost = db.apply(decision);
    assert_eq!(lost, vec!["old".to_owned()]);
    let stamp = db.stamp(201.0);
    let stopped = workers::stop_worker(&db.slice, "fresh", &stamp);
    db.apply(stopped);
    let listed: Vec<String> = workers::list_workers(&db.slice)
        .into_iter()
        .map(|worker| format!("{}:{}", worker.id, worker.status))
        .collect();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&"fresh:stopped".to_owned()));
    assert!(listed.contains(&"old:lost".to_owned()));
}

fn artifact(path: &str, created: f64, retention: &str, size: i64) -> Artifact {
    Artifact {
        id: derive_id(&[7; 32], path.len() as u32),
        workspace_id: derive_id(&[8; 32], 0),
        task_id: None,
        attempt_id: None,
        kind: "log".to_owned(),
        name: path.to_owned(),
        media_type: "text/plain".to_owned(),
        path: path.to_owned(),
        size_bytes: size,
        sha256: String::new(),
        retention: retention.to_owned(),
        purged_at: None,
        metadata: JsonObject::new(),
        created_at: at(created),
    }
}

#[test]
fn retention_purges_only_old_telemetry_inside_the_root() {
    let day = 86_400.0;
    let policy = RetentionPolicy {
        telemetry_payload_days: 7,
        run_log_days: 30,
    };
    let now = 60.0 * day;
    let artifacts = vec![
        artifact("old-log.txt", 10.0 * day, "telemetry", 100),
        artifact("kept-artifact.bin", 10.0 * day, "keep", 999),
        artifact("recent-log.txt", 50.0 * day, "telemetry", 7),
        artifact("../escape.txt", 10.0 * day, "telemetry", 1000),
    ];
    let stamp = stamp_only(now);
    let decision =
        retention::compact_artifacts(&artifacts, &policy, &stamp, |path| !path.starts_with(".."));
    assert_eq!(decision.reply.artifacts, 1);
    assert_eq!(decision.reply.bytes_freed, 100);
    assert_eq!(decision.writes.len(), 1);
    let (payload_cutoff, log_cutoff) = retention::cutoffs(&policy, stamp.at);
    assert!(payload_cutoff > log_cutoff);
}

#[test]
fn claim_filters_by_kind() {
    let mut db = Db::new();
    db.enqueue(0.0, &job("report"));
    assert!(db.claim("w", 0.0, &["noop"], 60).is_none());
    assert!(db.claim("w", 0.0, &["report"], 60).is_some());
}

#[test]
fn queue_stats_count_only_statuses_that_occur() {
    let mut db = Db::new();
    db.enqueue(0.0, &job("noop"));
    let counts: BTreeMap<String, usize> = queue::stats(&db.slice, None);
    assert_eq!(counts.get("queued"), Some(&1));
    assert_eq!(counts.len(), 1);
}
