//! GV-20: every case of `golden/queue/*.json` must match the queue decisions.
//!
//! The golden files were recorded by `tools/golden/gen_queue.py`, which drives the Python queue
//! through a temporary database with a fixed clock. This test repeats that driver against the
//! pure decisions of `wg_jobs::queue`: each harness step applies the decision's writes to an
//! in-memory `QueueSlice`, and each recorded step is rebuilt in the same JSON shape and compared.

use std::collections::BTreeMap;

use rpds::{RedBlackTreeSetSync, VectorSync};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use wg_jobs::limits::{LimitSettings, agent_key, repo_key, stack_key, tracker_key, verify_key};
use wg_jobs::queue::{self, BACKOFF_CAP_SECONDS, JobFilter, NewJob};
use wg_model::entity::JsonObject;
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write, derive_id};
use wg_types::{Actor, ErrorCode, Id, JobStatus, Timestamp, WgError};

const EPOCH_UNIX_SECONDS: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z, the generator's EPOCH

fn ok<T>(result: Result<T, WgError>) -> T {
    result.unwrap_or_else(|err| panic!("unexpected error: {err:?}"))
}

fn epoch() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(EPOCH_UNIX_SECONDS).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// The instant `seconds` after the generator's epoch.
fn at(seconds: f64) -> Timestamp {
    let moment = epoch() + Duration::microseconds((seconds * 1_000_000.0).round() as i64);
    ok(Timestamp::from_offset_datetime(moment))
}

/// Seconds after the generator's epoch, as the golden files record them.
fn off(instant: Timestamp) -> f64 {
    (instant.to_offset_datetime() - epoch()).as_seconds_f64()
}

/// Python's `repr` of a float that holds a whole number, as used in the generator's messages.
fn py_float(value: f64) -> String {
    format!("{value:.1}")
}

/// The limits the generator's configuration gives (`_config`).
fn limits() -> LimitSettings {
    LimitSettings {
        gpu_concurrency: 1,
        per_agent_concurrency: 1,
        per_repository_concurrency: 1,
        verify_concurrency: 2,
        max_concurrent_runs: 2,
    }
}

/// The error name the generator records: the Python exception class.
fn error_name(err: &WgError) -> &'static str {
    match err.code {
        ErrorCode::Conflict => "ConflictError",
        ErrorCode::NotFound => "NotFoundError",
        _ => "OtherError",
    }
}

/// `_attempt`: `{"ok": value}` or `{"error": exception name}`.
fn outcome<T>(result: Result<T, WgError>, value: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(found) => json!({ "ok": value(found) }),
        Err(err) => json!({ "error": error_name(&err) }),
    }
}

fn unit(_: ()) -> Value {
    Value::Null
}

fn job(kind: &str, keys: Vec<String>) -> NewJob {
    NewJob {
        resource_keys: keys,
        ..NewJob::new(kind)
    }
}

fn payload(entries: &[(&str, Value)]) -> JsonObject {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

/// A fresh database with the generator's helpers: labels, a stamp per step, and a row view.
struct Harness {
    slice: QueueSlice,
    labels: BTreeMap<Id, String>,
    seq: u32,
    limits: LimitSettings,
}

impl Harness {
    fn new() -> Self {
        Harness {
            slice: QueueSlice::default(),
            labels: BTreeMap::new(),
            seq: 0,
            limits: limits(),
        }
    }

    /// A stamp at `seconds` with entropy unique to this step, so every id of the run differs.
    fn stamp(&mut self, seconds: f64) -> Stamp {
        self.seq += 1;
        let mut entropy = [0u8; 32];
        entropy[..4].copy_from_slice(&self.seq.to_be_bytes());
        Stamp {
            at: at(seconds),
            entropy,
            actor: ok(Actor::parse("system:golden")),
            permissions: RedBlackTreeSetSync::new_sync(),
            correlation: derive_id(&entropy, 0).into(),
            causation: None,
            attempt: None,
            automation_depth: 0,
        }
    }

    fn commit(&mut self, writes: &VectorSync<Write>) {
        self.slice = self.slice.apply_all(writes.iter());
    }

    fn apply<R>(&mut self, decision: Decision<R>) -> R {
        self.commit(&decision.writes);
        decision.reply
    }

    fn apply_result<R>(&mut self, result: Result<Decision<R>, WgError>) -> Result<R, WgError> {
        result.map(|decision| self.apply(decision))
    }

    /// Enqueues a job the generator labels, returning its id.
    fn enqueue(&mut self, seconds: f64, label: &str, new_job: &NewJob) -> Id {
        let id = self.enqueue_unlabelled(seconds, new_job);
        self.labels.insert(id, label.to_owned());
        id
    }

    fn enqueue_unlabelled(&mut self, seconds: f64, new_job: &NewJob) -> Id {
        let stamp = self.stamp(seconds);
        let decision = queue::enqueue(&self.slice, new_job, &stamp);
        self.apply(decision)
    }

    fn label(&self, id: Id) -> Value {
        json!(self.labels.get(&id).map_or("unlabelled", String::as_str))
    }

    fn label_or_null(&self, id: Option<Id>) -> Value {
        id.map_or(Value::Null, |found| self.label(found))
    }

    /// The `claim` step of the generator.
    fn claim(&mut self, worker: &str, seconds: f64, kinds: &[&str], lease_seconds: i64) -> Value {
        let stamp = self.stamp(seconds);
        let decision = queue::claim(
            &self.slice,
            worker,
            kinds,
            lease_seconds,
            &self.limits,
            &stamp,
        );
        match self.apply(decision) {
            None => json!({ "op": "claim", "worker": worker, "at": seconds, "got": null }),
            Some(claimed) => json!({
                "op": "claim",
                "worker": worker,
                "at": seconds,
                "got": self.label(claimed.id),
                "attempt": claimed.attempt,
                "lease_expires_at": off(claimed.lease_expires_at),
            }),
        }
    }

    fn claim_default(&mut self, worker: &str, seconds: f64) -> Value {
        self.claim(worker, seconds, &[], 60)
    }

    fn claim_peek(&mut self, seconds: f64, kinds: &[&str]) -> Value {
        let stamp = self.stamp(seconds);
        let found = queue::claim_peek(&self.slice, kinds, &stamp);
        json!({ "op": "claim_peek", "got": self.label_or_null(found) })
    }

    fn row(&self, id: Id) -> Value {
        let found = ok(queue::get(&self.slice, id));
        json!({
            "label": self.label(id),
            "status": found.status.as_str(),
            "attempts": found.attempts,
            "max_attempts": found.max_attempts,
            "run_at": off(found.run_at),
            "finished_at": found.finished_at.map(off),
            "error_kind": found.error_kind,
            "last_error": found.last_error,
            "cancel_requested": found.cancel_requested,
            "resource_keys": found.resource_keys.iter().cloned().collect::<Vec<String>>(),
        })
    }

    fn runs(&self, id: Id) -> Value {
        Value::Array(
            queue::runs(&self.slice, id)
                .into_iter()
                .map(|run| {
                    json!({
                        "attempt_number": run.attempt_number,
                        "worker": run.worker_id,
                        "status": run.status,
                        "error": run.error,
                        "finished_at": run.finished_at.map(off),
                    })
                })
                .collect(),
        )
    }

    fn fail(
        &mut self,
        seconds: f64,
        id: Id,
        worker: &str,
        error: &str,
        retryable: bool,
        kind: Option<&str>,
    ) -> Result<JobStatus, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::fail(&self.slice, id, worker, error, retryable, kind, &stamp);
        self.apply_result(decision)
    }

    fn heartbeat(
        &mut self,
        seconds: f64,
        id: Id,
        worker: &str,
        lease_seconds: i64,
    ) -> Result<bool, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::heartbeat(&self.slice, id, worker, lease_seconds, &stamp);
        self.apply_result(decision)
    }

    fn complete(
        &mut self,
        seconds: f64,
        id: Id,
        worker: &str,
        result: Option<JsonObject>,
    ) -> Result<(), WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::complete(&self.slice, id, worker, result, &stamp);
        self.apply_result(decision)
    }

    fn mark_cancelled(
        &mut self,
        seconds: f64,
        id: Id,
        worker: &str,
        reason: &str,
    ) -> Result<(), WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::mark_cancelled(&self.slice, id, worker, reason, &stamp);
        self.apply_result(decision)
    }

    fn cancel(&mut self, seconds: f64, id: Id) -> Result<JobStatus, WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::cancel(&self.slice, id, &stamp);
        self.apply_result(decision)
    }

    fn retry_dead(&mut self, seconds: f64, id: Id) -> Result<(), WgError> {
        let stamp = self.stamp(seconds);
        let decision = queue::retry_dead(&self.slice, id, &stamp);
        self.apply_result(decision)
    }

    fn recover(&mut self, seconds: f64) -> Vec<Id> {
        let stamp = self.stamp(seconds);
        let decision = queue::recover_expired(&self.slice, &stamp);
        self.apply(decision)
    }

    fn ids_labels(&self, ids: &[Id]) -> Value {
        Value::Array(ids.iter().map(|id| self.label(*id)).collect())
    }
}

fn case(name: &str, steps: Vec<Value>) -> Value {
    json!({ "name": name, "steps": steps })
}

fn claim_order_cases() -> Vec<Value> {
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let gpu_1 = h.enqueue(0.0, "gpu-1", &job("noop", vec!["gpu".to_owned()]));
    h.enqueue(1.0, "gpu-2", &job("noop", vec!["gpu".to_owned()]));
    h.enqueue(2.0, "agent-alice-1", &job("noop", vec![agent_key("alice")]));
    let repo_1 = h.enqueue(
        3.0,
        "repo-r1-1",
        &NewJob {
            priority: 50,
            ..job("noop", vec![repo_key("r1")])
        },
    );
    h.enqueue(
        4.0,
        "verify-r1-1",
        &job("noop", vec![verify_key("r1"), repo_key("r1")]),
    );
    h.enqueue(5.0, "tracker-c1-1", &job("noop", vec![tracker_key("c1")]));
    h.enqueue(
        6.0,
        "stack-s1-r1-1",
        &job("noop", vec![stack_key("s1", "r1")]),
    );
    h.enqueue(
        7.0,
        "unlimited-1",
        &NewJob {
            priority: 200,
            ..NewJob::new("noop")
        },
    );
    h.enqueue(8.0, "verify-r2-1", &job("noop", vec![verify_key("r2")]));
    h.enqueue(9.0, "verify-r2-2", &job("noop", vec![verify_key("r2")]));
    h.enqueue(10.0, "verify-r2-3", &job("noop", vec![verify_key("r2")]));
    h.enqueue(
        11.0,
        "gpu-3",
        &job("noop", vec!["gpu".to_owned(), "run".to_owned()]),
    );
    h.enqueue(
        12.0,
        "future",
        &NewJob {
            priority: 1,
            run_at: Some(at(600.0)),
            ..NewJob::new("noop")
        },
    );

    // Drain every runnable job with nothing completed: the limits decide what is taken.
    for _ in 0..12 {
        let step = h.claim_default("w1", 20.0);
        let exhausted = step["got"].is_null();
        steps.push(step);
        if exhausted {
            break;
        }
    }
    steps.push(h.claim_peek(20.0, &[]));

    // Completing the GPU holder frees the gpu slot for the next gpu job.
    let result = h.complete(20.0, gpu_1, "w1", None);
    steps.push(json!({
        "op": "complete",
        "label": "gpu-1",
        "result": outcome(result, unit),
    }));
    steps.push(h.claim_default("w1", 20.0));

    // Completing the repository job frees repo:r1, which unblocks the verify job.
    let result = h.complete(20.0, repo_1, "w1", None);
    steps.push(json!({
        "op": "complete",
        "label": "repo-r1-1",
        "result": outcome(result, unit),
    }));
    steps.push(h.claim_default("w1", 20.0));

    // Jobs not yet eligible wait for their run_at; the kind filter applies first.
    h.enqueue(600.0, "report-1", &job("report", Vec::new()));
    steps.push(h.claim("w2", 600.0, &["noop"], 60));
    steps.push(h.claim("w2", 600.0, &["report"], 60));
    steps.push(h.claim_default("w2", 600.0));
    steps.push(h.claim_default("w2", 600.0));

    vec![case("limits_and_order", steps)]
}

fn backoff_cases() -> Vec<Value> {
    let mut table = Vec::new();
    for base in [0.0, 0.5, 2.0, 10.0] {
        for attempt in [0i64, 1, 2, 3, 4, 8, 12, 13] {
            table.push(json!({
                "base": base,
                "attempt": attempt,
                "delay": queue::backoff_delay(base, attempt, BACKOFF_CAP_SECONDS),
            }));
        }
    }
    let mut capped = Vec::new();
    for base in [2.0, 10.0] {
        for attempt in 1..=5i64 {
            capped.push(json!({
                "base": base,
                "attempt": attempt,
                "cap": 30.0,
                "delay": queue::backoff_delay(base, attempt, 30.0),
            }));
        }
    }

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "retry-job",
        &NewJob {
            max_attempts: 4,
            backoff_base_seconds: 2.0,
            ..NewJob::new("noop")
        },
    );
    let mut clock = 0.0;
    for _ in 0..4 {
        let now = clock;
        steps.push(h.claim_default("w", now));
        let status = ok(h.fail(
            now,
            job_id,
            "w",
            &format!("boom at {}", py_float(now)),
            true,
            None,
        ));
        let run_at = off(ok(queue::get(&h.slice, job_id)).run_at);
        steps.push(json!({
            "op": "fail",
            "status": status.as_str(),
            "row": h.row(job_id),
        }));
        if status != JobStatus::Queued {
            break;
        }
        // Just before run_at nothing is claimable; at run_at it is.
        steps.push(h.claim_default("w", run_at - 1.0));
        clock = run_at;
    }
    steps.push(json!({ "op": "runs", "runs": h.runs(job_id) }));

    // Zero base: a retried job is immediately claimable.
    let zero = h.enqueue(
        100.0,
        "zero-base",
        &NewJob {
            max_attempts: 3,
            backoff_base_seconds: 0.0,
            ..NewJob::new("noop")
        },
    );
    let _ = h.claim_default("w", 100.0);
    let status = ok(h.fail(100.0, zero, "w", "x", true, None));
    steps.push(json!({
        "op": "fail",
        "status": status.as_str(),
        "row": h.row(zero),
    }));
    steps.push(h.claim_default("w", 100.0));

    vec![
        json!({ "name": "backoff_delay_table", "vectors": table }),
        json!({ "name": "backoff_delay_capped", "vectors": capped }),
        case("retry_schedule", steps),
    ]
}

fn lease_cases() -> Vec<Value> {
    let mut cases = Vec::new();

    // A lease is recovered only strictly after lease_expires_at.
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "lease-job",
        &NewJob {
            max_attempts: 3,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim("crashed", 0.0, &[], 5));
    let recovered = h.recover(5.0);
    steps.push(json!({ "op": "recover", "at": 5, "ids": h.ids_labels(&recovered) }));
    let recovered = h.recover(6.0);
    steps.push(json!({ "op": "recover", "at": 6, "ids": h.ids_labels(&recovered) }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    let result = h.heartbeat(6.0, job_id, "crashed", 60);
    steps.push(json!({
        "op": "heartbeat",
        "worker": "crashed",
        "result": outcome(result, |found| json!(found)),
    }));
    steps.push(h.claim_default("w2", 6.0));
    steps.push(json!({ "op": "runs", "runs": h.runs(job_id) }));
    cases.push(case("expiry_boundary_and_redelivery", steps));

    // Heartbeats extend the lease and report cancellation.
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "heartbeat-job",
        &NewJob {
            max_attempts: 3,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim("w1", 0.0, &[], 5));
    let result = h.heartbeat(4.0, job_id, "w1", 10);
    steps.push(json!({
        "op": "heartbeat",
        "at": 4,
        "result": outcome(result, |found| json!(found)),
    }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    let recovered = h.recover(13.0);
    steps.push(json!({ "op": "recover", "at": 13, "ids": h.ids_labels(&recovered) }));
    let recovered = h.recover(15.0);
    steps.push(json!({ "op": "recover", "at": 15, "ids": h.ids_labels(&recovered) }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    cases.push(case("heartbeat_extends_lease", steps));

    // Lease expiry on the last attempt goes to the dead letter.
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "last-attempt",
        &NewJob {
            max_attempts: 1,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim("w1", 0.0, &[], 5));
    let recovered = h.recover(10.0);
    steps.push(json!({ "op": "recover", "at": 10, "ids": h.ids_labels(&recovered) }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    steps.push(json!({ "op": "runs", "runs": h.runs(job_id) }));
    cases.push(case("expiry_on_last_attempt_is_dead", steps));

    // Only the lease owner may heartbeat or complete; unknown ids fail too.
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(0.0, "owned-job", &NewJob::new("noop"));
    steps.push(h.claim_default("w1", 0.0));
    let result = h.heartbeat(0.0, job_id, "w2", 60);
    steps.push(json!({
        "op": "heartbeat",
        "worker": "w2",
        "result": outcome(result, |found| json!(found)),
    }));
    let result = h.complete(0.0, job_id, "w2", None);
    steps.push(json!({
        "op": "complete",
        "worker": "w2",
        "result": outcome(result, unit),
    }));
    let unknown = derive_id(&[0xAB; 32], 7);
    let result = h.heartbeat(0.0, unknown, "w1", 60);
    steps.push(json!({
        "op": "heartbeat",
        "worker": "w1",
        "job": "unknown",
        "result": outcome(result, |found| json!(found)),
    }));
    let result = queue::get(&h.slice, unknown).map(|found| found.status);
    steps.push(json!({
        "op": "get",
        "job": "unknown",
        "result": outcome(result, |status| json!(status.as_str())),
    }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    cases.push(case("ownership_checks", steps));

    cases
}

fn dead_letter_cases() -> Vec<Value> {
    let mut cases = Vec::new();

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "two-tries",
        &NewJob {
            max_attempts: 2,
            backoff_base_seconds: 10.0,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim_default("w", 0.0));
    let status = ok(h.fail(0.0, job_id, "w", "first failure", true, None));
    steps.push(json!({
        "op": "fail",
        "status": status.as_str(),
        "row": h.row(job_id),
    }));
    steps.push(h.claim_default("w", 10.0));
    let status = ok(h.fail(10.0, job_id, "w", "second failure", true, None));
    steps.push(json!({
        "op": "fail",
        "status": status.as_str(),
        "row": h.row(job_id),
    }));
    steps.push(json!({ "op": "runs", "runs": h.runs(job_id) }));
    let result = h.retry_dead(20.0, job_id);
    steps.push(json!({ "op": "retry_dead", "result": outcome(result, unit) }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    let result = h.retry_dead(20.0, job_id);
    steps.push(json!({
        "op": "retry_dead",
        "note": "queued job",
        "result": outcome(result, unit),
    }));
    steps.push(h.claim_default("w", 20.0));
    steps.push(json!({ "op": "stats", "stats": queue::stats(&h.slice, None) }));
    cases.push(case("retry_exhaustion_then_manual_retry", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "non-retryable",
        &NewJob {
            max_attempts: 5,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim_default("w", 0.0));
    let status = ok(h.fail(0.0, job_id, "w", "bad input", false, Some("validation")));
    steps.push(json!({
        "op": "fail",
        "status": status.as_str(),
        "row": h.row(job_id),
    }));
    steps.push(h.claim_default("w", 0.0));
    cases.push(case("non_retryable_goes_dead", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "long-error",
        &NewJob {
            max_attempts: 5,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim_default("w", 0.0));
    let long_error = "x".repeat(25_000);
    let status = ok(h.fail(0.0, job_id, "w", &long_error, true, None));
    steps.push(json!({ "op": "fail", "status": status.as_str() }));
    let row = ok(queue::get(&h.slice, job_id))
        .last_error
        .clone()
        .unwrap_or_default();
    let run_error = queue::runs(&h.slice, job_id)
        .first()
        .and_then(|run| run.error.clone())
        .unwrap_or_default();
    steps.push(json!({
        "op": "lengths",
        "last_error": row.chars().count(),
        "run_error": run_error.chars().count(),
    }));
    cases.push(case("error_text_truncation", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let job_id = h.enqueue(
        0.0,
        "cancel-during-run",
        &NewJob {
            max_attempts: 5,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim_default("w", 0.0));
    let status = ok(h.cancel(0.0, job_id));
    steps.push(json!({ "op": "cancel", "status": status.as_str() }));
    let status = ok(h.fail(0.0, job_id, "w", "boom", true, None));
    steps.push(json!({
        "op": "fail",
        "status": status.as_str(),
        "row": h.row(job_id),
    }));
    steps.push(json!({ "op": "runs", "runs": h.runs(job_id) }));
    let result = h.retry_dead(0.0, job_id);
    steps.push(json!({
        "op": "retry_dead",
        "note": "cancelled job",
        "result": outcome(result, unit),
    }));
    steps.push(json!({ "op": "row", "row": h.row(job_id) }));
    cases.push(case("cancel_requested_failure_is_cancelled", steps));

    cases
}

fn idempotency_cases() -> Vec<Value> {
    let mut h = Harness::new();
    let mut steps = Vec::new();
    let keyed = |key: &str, kind: &str, keys: Vec<String>| NewJob {
        idempotency_key: Some(key.to_owned()),
        resource_keys: keys,
        ..NewJob::new(kind)
    };

    let first = h.enqueue(
        0.0,
        "first",
        &NewJob {
            payload: payload(&[("x", json!(1))]),
            idempotency_key: Some("k1".to_owned()),
            ..NewJob::new("noop")
        },
    );
    let again = h.enqueue_unlabelled(
        0.0,
        &NewJob {
            payload: payload(&[("x", json!(2))]),
            ..keyed("k1", "noop", Vec::new())
        },
    );
    let stored = ok(queue::get(&h.slice, first)).payload.clone();
    steps.push(json!({
        "op": "enqueue",
        "key": "k1",
        "same_job": again == first,
        "label": h.label(again),
        "payload": stored,
    }));
    steps.push(h.claim_default("w", 0.0));
    let result = h.complete(0.0, first, "w", Some(payload(&[("done", json!(true))])));
    let _ = outcome(result, unit);
    let after = h.enqueue_unlabelled(
        0.0,
        &NewJob {
            payload: payload(&[("x", json!(3))]),
            ..keyed("k1", "noop", Vec::new())
        },
    );
    let status = ok(queue::get(&h.slice, first)).status;
    steps.push(json!({
        "op": "enqueue_after_success",
        "key": "k1",
        "same_job": after == first,
        "status": status.as_str(),
    }));
    let (listed, total) = queue::list(
        &h.slice,
        &JobFilter {
            limit: Some(50),
            ..JobFilter::default()
        },
    );
    let mut labels: Vec<String> = listed
        .iter()
        .map(|found| {
            h.labels
                .get(&found.id)
                .cloned()
                .unwrap_or_else(|| "unlabelled".to_owned())
        })
        .collect();
    labels.sort();
    steps.push(json!({ "op": "list", "total": total, "labels": labels }));

    let keep = h.enqueue(
        0.0,
        "keep-resources",
        &keyed("k2", "noop", vec!["gpu".to_owned()]),
    );
    let dup = h.enqueue_unlabelled(0.0, &keyed("k2", "noop", Vec::new()));
    steps.push(json!({
        "op": "enqueue",
        "key": "k2",
        "same_job": dup == keep,
        "resource_keys": h.row(keep)["resource_keys"],
    }));

    let kind_keep = h.enqueue(0.0, "report-kind", &keyed("k3", "report", Vec::new()));
    let kind_dup = h.enqueue_unlabelled(0.0, &keyed("k3", "noop", Vec::new()));
    steps.push(json!({
        "op": "enqueue",
        "key": "k3",
        "same_job": kind_dup == kind_keep,
        "kind": ok(queue::get(&h.slice, kind_keep)).kind,
    }));

    let cancelled = h.enqueue(0.0, "cancel-me", &keyed("k4", "noop", Vec::new()));
    let _ = ok(h.cancel(0.0, cancelled));
    let dup_cancelled = h.enqueue_unlabelled(0.0, &keyed("k4", "noop", Vec::new()));
    let status = ok(queue::get(&h.slice, cancelled)).status;
    steps.push(json!({
        "op": "enqueue_after_cancel",
        "key": "k4",
        "same_job": dup_cancelled == cancelled,
        "status": status.as_str(),
    }));

    let anon_a = h.enqueue(0.0, "anon-a", &NewJob::new("noop"));
    let anon_b = h.enqueue(0.0, "anon-b", &NewJob::new("noop"));
    steps.push(json!({
        "op": "enqueue_without_key",
        "distinct": anon_a != anon_b,
        "labels": [h.label(anon_a), h.label(anon_b)],
    }));

    vec![case("idempotency_keys", steps)]
}

fn cancellation_cases() -> Vec<Value> {
    let mut cases = Vec::new();

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let queued = h.enqueue(0.0, "queued-cancel", &NewJob::new("noop"));
    let status = ok(h.cancel(0.0, queued));
    steps.push(json!({
        "op": "cancel",
        "status": status.as_str(),
        "row": h.row(queued),
    }));
    steps.push(h.claim_default("w", 0.0));
    steps.push(h.claim_peek(0.0, &[]));
    cases.push(case("queued_cancel_is_never_claimed", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let running = h.enqueue(0.0, "running-complete", &NewJob::new("noop"));
    steps.push(h.claim_default("w1", 0.0));
    let status = ok(h.cancel(0.0, running));
    steps.push(json!({ "op": "cancel", "status": status.as_str() }));
    let result = h.heartbeat(0.0, running, "w1", 60);
    steps.push(json!({
        "op": "heartbeat",
        "result": outcome(result, |found| json!(found)),
    }));
    let result = h.complete(0.0, running, "w1", Some(payload(&[("ok", json!(true))])));
    steps.push(json!({
        "op": "complete",
        "result": outcome(result, unit),
    }));
    steps.push(json!({ "op": "row", "row": h.row(running) }));
    steps.push(json!({ "op": "runs", "runs": h.runs(running) }));
    cases.push(case(
        "running_cancel_is_advisory_until_worker_reports",
        steps,
    ));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let operator = h.enqueue(0.0, "operator-stop", &NewJob::new("noop"));
    steps.push(h.claim_default("w1", 0.0));
    let result = h.mark_cancelled(0.0, operator, "w2", "cancelled");
    steps.push(json!({
        "op": "mark_cancelled",
        "wrong_worker": outcome(result, unit),
    }));
    let result = h.mark_cancelled(0.0, operator, "w1", "stopped by operator");
    steps.push(json!({
        "op": "mark_cancelled",
        "result": outcome(result, unit),
    }));
    steps.push(json!({ "op": "row", "row": h.row(operator) }));
    steps.push(json!({ "op": "runs", "runs": h.runs(operator) }));
    cases.push(case("mark_cancelled_records_reason", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let expired = h.enqueue(
        0.0,
        "cancel-then-expire",
        &NewJob {
            max_attempts: 5,
            ..NewJob::new("noop")
        },
    );
    steps.push(h.claim("w1", 0.0, &[], 5));
    let status = ok(h.cancel(0.0, expired));
    steps.push(json!({ "op": "cancel", "status": status.as_str() }));
    let recovered = h.recover(10.0);
    steps.push(json!({ "op": "recover", "ids": h.ids_labels(&recovered) }));
    steps.push(json!({ "op": "row", "row": h.row(expired) }));
    cases.push(case("cancel_requested_expiry_is_cancelled", steps));

    let mut h = Harness::new();
    let mut steps = Vec::new();
    let done = h.enqueue(0.0, "already-done", &NewJob::new("noop"));
    steps.push(h.claim_default("w", 0.0));
    let _ = h.complete(0.0, done, "w", None);
    let status = ok(h.cancel(0.0, done));
    steps.push(json!({ "op": "cancel_succeeded", "status": status.as_str() }));
    let unknown = derive_id(&[0xCD; 32], 9);
    let result = h.cancel(0.0, unknown);
    steps.push(json!({
        "op": "cancel_unknown",
        "result": outcome(result, |found| json!(found.as_str())),
    }));
    steps.push(json!({ "op": "row", "row": h.row(done) }));
    cases.push(case("cancel_terminal_and_unknown", steps));

    cases
}

fn golden(group: &str) -> Value {
    let path = format!(
        "{}/../../golden/queue/{group}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn check_group(group: &str, built: Vec<Value>) {
    let recorded = golden(group)["cases"].clone();
    let expected = recorded.as_array().cloned().unwrap_or_default();
    assert_eq!(
        built.len(),
        expected.len(),
        "{group}: number of cases differs"
    );
    for (got, want) in built.iter().zip(expected.iter()) {
        let name = want["name"].as_str().unwrap_or_default();
        assert_eq!(got["name"], want["name"], "{group}: case order");
        assert_eq!(got, want, "{group}/{name}");
    }
}

#[test]
fn claim_order_matches_golden() {
    check_group("claim_order", claim_order_cases());
}

#[test]
fn backoff_matches_golden() {
    check_group("backoff", backoff_cases());
}

#[test]
fn leases_match_golden() {
    check_group("leases", lease_cases());
}

#[test]
fn dead_letter_matches_golden() {
    check_group("dead_letter", dead_letter_cases());
}

#[test]
fn idempotency_matches_golden() {
    check_group("idempotency", idempotency_cases());
}

#[test]
fn cancellation_matches_golden() {
    check_group("cancellation", cancellation_cases());
}
