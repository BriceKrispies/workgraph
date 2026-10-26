//! Queue-family checks (SH-08) against real SQLite: the atomic claim (order, lease and run rows,
//! resource claims, kind and due filters, resource limits), the paged job listing with its count,
//! job runs in start order, status counts, the worker listing cap and system cursors.
//!
//! The database is built from `contracts/schema-0011.sql`, as the other SH checks build it.

use std::error::Error;
use std::path::PathBuf;

use rpds::{RedBlackTreeSetSync, VectorSync};
use wg_jobs::limits::LimitSettings;
use wg_jobs::queue::JobFilter;
use wg_model::entity::{Job, JobRun, JsonObject, Real, SystemCursor, Worker};
use wg_model::stamp::Stamp;
use wg_model::{Row, Write, derive_id};
use wg_store::{Location, Raw, Statement, Store};
use wg_types::{Actor, Id, JobStatus, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn schema() -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let wanted: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|block| {
            (block.starts_with("CREATE TABLE") && !block.starts_with("CREATE TABLE 'task_fts_"))
                || block.starts_with("CREATE VIRTUAL TABLE")
                || block.starts_with("CREATE TRIGGER")
        })
        .collect();
    Ok(wanted.join(";\n"))
}

fn store() -> Result<Store, Box<dyn Error>> {
    let mut store = Store::open(&Location::Memory)?;
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(store)
}

fn id(n: u32) -> Id {
    derive_id(&[7u8; 32], n)
}

/// The workspace every job fixture belongs to unless a test changes it.
fn workspace() -> Id {
    id(100)
}

fn at(minute: u8, second: u8) -> Timestamp {
    ok(Timestamp::from_components(
        2026, 10, 25, 12, minute, second, 0,
    ))
}

/// A stamp at `now` whose ids differ per `seq`.
fn stamp(seq: u8, now: Timestamp) -> Stamp {
    let mut entropy = [0u8; 32];
    entropy[0] = seq;
    entropy[1] = 1;
    Stamp {
        at: now,
        entropy,
        actor: ok(Actor::parse("system:test")),
        permissions: RedBlackTreeSetSync::new_sync(),
        correlation: derive_id(&entropy, 0),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn limits(gpu: i64) -> LimitSettings {
    LimitSettings {
        gpu_concurrency: gpu,
        per_agent_concurrency: 1,
        per_repository_concurrency: 1,
        verify_concurrency: 1,
        max_concurrent_runs: 10,
    }
}

fn keys(names: &[&str]) -> VectorSync<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A queued job of `kind` in the test workspace, with no resource keys.
fn job(n: u32, kind: &str, priority: i64, run_at: Timestamp, created_at: Timestamp) -> Job {
    Job {
        id: id(n),
        workspace_id: Some(workspace()),
        kind: kind.to_owned(),
        status: JobStatus::Queued,
        priority,
        payload: JsonObject::new(),
        result: None,
        run_at,
        attempts: 0,
        max_attempts: 5,
        backoff_base_seconds: Real(2.0),
        timeout_seconds: None,
        lease_owner: None,
        lease_expires_at: None,
        heartbeat_at: None,
        started_at: None,
        finished_at: None,
        cancel_requested: false,
        idempotency_key: None,
        resource_keys: VectorSync::new_sync(),
        last_error: None,
        error_kind: None,
        correlation_id: None,
        causation_id: None,
        task_id: None,
        attempt_id: None,
        created_by: "system".to_owned(),
        created_at,
        updated_at: created_at,
    }
}

fn put(store: &mut Store, row: impl Into<Row>) {
    ok(store.write(|session| session.apply_write(&Write::insert(row))));
}

fn ids(jobs: &[Job]) -> Vec<Id> {
    jobs.iter().map(|job| job.id).collect()
}

#[test]
fn claims_follow_priority_then_run_time_then_creation() -> TestResult {
    let mut store = store()?;
    put(&mut store, job(1, "a", 100, at(1, 0), at(1, 0)));
    put(&mut store, job(2, "a", 50, at(1, 5), at(1, 5)));
    put(&mut store, job(3, "a", 50, at(1, 0), at(1, 6)));
    let now = at(2, 0);
    let mut order = Vec::new();
    for seq in 1..=3 {
        let claimed = ok(store.claim_job("w1", &[], 60, &limits(10), &stamp(seq, now)));
        order.push(claimed.map(|claim| claim.id));
    }
    assert_eq!(order, vec![Some(id(3)), Some(id(2)), Some(id(1))]);
    let none = ok(store.claim_job("w1", &[], 60, &limits(10), &stamp(4, now)));
    assert!(none.is_none());
    Ok(())
}

#[test]
fn claim_leases_the_job_and_writes_its_run_and_resource_claims() -> TestResult {
    let mut store = store()?;
    put(
        &mut store,
        Job {
            resource_keys: keys(&["gpu", "repo:r1"]),
            ..job(1, "a", 100, at(1, 0), at(1, 0))
        },
    );
    let now = at(2, 0);
    let claimed = ok(store.claim_job("w1", &[], 60, &limits(10), &stamp(1, now)))
        .ok_or("nothing was claimed")?;
    assert_eq!(claimed.id, id(1));
    assert_eq!(claimed.attempt, 1);
    assert_eq!(claimed.lease_expires_at, at(3, 0));

    let row = ok(store.read(|session| session.job(id(1)))).ok_or("the claimed job is missing")?;
    assert_eq!(row.status, JobStatus::Running);
    assert_eq!(row.lease_owner.as_deref(), Some("w1"));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.started_at, Some(now));
    assert_eq!(row.heartbeat_at, Some(now));

    let runs = ok(store.read(|session| session.job_runs(id(1))));
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].attempt_number, 1);
    assert_eq!(runs[0].worker_id, "w1");
    assert_eq!(runs[0].status, "running");

    let claims = ok(store.read(|session| {
        session.rows(&Statement {
            sql: "SELECT resource_key FROM job_resource_claims ORDER BY resource_key".to_owned(),
            params: Vec::new(),
        })
    }));
    assert_eq!(
        claims,
        vec![
            vec![Raw::Text("gpu".to_owned())],
            vec![Raw::Text("repo:r1".to_owned())]
        ]
    );
    Ok(())
}

#[test]
fn claim_skips_cancel_requested_future_and_other_kinds() -> TestResult {
    let mut store = store()?;
    put(&mut store, job(1, "a", 100, at(1, 0), at(1, 0)));
    put(
        &mut store,
        Job {
            cancel_requested: true,
            ..job(2, "b", 100, at(1, 0), at(1, 1))
        },
    );
    put(&mut store, job(3, "a", 100, at(2, 30), at(1, 2)));
    put(&mut store, job(4, "c", 100, at(1, 0), at(1, 3)));
    let now = at(2, 0);
    let kinds = ["a", "b"];
    let first = ok(store.claim_job("w1", &kinds, 60, &limits(10), &stamp(1, now)));
    assert_eq!(first.map(|claim| claim.id), Some(id(1)));
    let second = ok(store.claim_job("w1", &kinds, 60, &limits(10), &stamp(2, now)));
    assert!(
        second.is_none(),
        "cancelled, not-yet-due and other-kind jobs are not claimed"
    );
    Ok(())
}

#[test]
fn resource_limits_skip_candidates_whose_keys_are_full() -> TestResult {
    let mut store = store()?;
    put(
        &mut store,
        Job {
            resource_keys: keys(&["gpu"]),
            ..job(1, "a", 1, at(1, 0), at(1, 0))
        },
    );
    put(
        &mut store,
        Job {
            resource_keys: keys(&["gpu"]),
            ..job(2, "a", 2, at(1, 0), at(1, 1))
        },
    );
    put(&mut store, job(3, "a", 3, at(1, 0), at(1, 2)));
    let now = at(2, 0);
    let limit = limits(1);
    let first = ok(store.claim_job("w1", &[], 60, &limit, &stamp(1, now)));
    assert_eq!(first.map(|claim| claim.id), Some(id(1)));
    // The gpu key is held by job 1, so job 2 waits and the unkeyed job 3 is taken.
    let second = ok(store.claim_job("w2", &[], 60, &limit, &stamp(2, now)));
    assert_eq!(second.map(|claim| claim.id), Some(id(3)));
    let third = ok(store.claim_job("w3", &[], 60, &limit, &stamp(3, now)));
    assert!(third.is_none());
    let waiting = ok(store.read(|session| session.job(id(2)))).ok_or("job 2 is missing")?;
    assert_eq!(waiting.status, JobStatus::Queued);
    Ok(())
}

#[test]
fn job_listing_pages_newest_first_and_counts_matches() -> TestResult {
    let mut store = store()?;
    for n in 1..=5u32 {
        let created = at(1, 10 + u8::try_from(n)?);
        let row = if n == 5 {
            Job {
                workspace_id: None,
                ..job(n, "a", 100, created, created)
            }
        } else if n == 2 {
            Job {
                status: JobStatus::Succeeded,
                ..job(n, "a", 100, created, created)
            }
        } else {
            job(n, "a", 100, created, created)
        };
        put(&mut store, row);
    }

    // Jobs with no workspace are listed with every workspace, so all five match.
    let page = JobFilter {
        workspace_id: Some(workspace()),
        offset: 1,
        limit: Some(2),
        ..Default::default()
    };
    let (jobs, total) = ok(store.read(|session| session.list_jobs(&page)));
    assert_eq!(total, 5);
    assert_eq!(ids(&jobs), vec![id(4), id(3)]);

    let rest = JobFilter {
        offset: 3,
        limit: None,
        ..Default::default()
    };
    let (jobs, total) = ok(store.read(|session| session.list_jobs(&rest)));
    assert_eq!(total, 5);
    assert_eq!(ids(&jobs), vec![id(2), id(1)]);

    let succeeded = JobFilter {
        statuses: &[JobStatus::Succeeded],
        ..Default::default()
    };
    let (jobs, total) = ok(store.read(|session| session.list_jobs(&succeeded)));
    assert_eq!(total, 1);
    assert_eq!(ids(&jobs), vec![id(2)]);

    let other_workspace = JobFilter {
        workspace_id: Some(id(999)),
        ..Default::default()
    };
    let (jobs, total) = ok(store.read(|session| session.list_jobs(&other_workspace)));
    assert_eq!(total, 1);
    assert_eq!(ids(&jobs), vec![id(5)]);

    let no_kind = JobFilter {
        kind: Some("zzz"),
        ..Default::default()
    };
    let (jobs, total) = ok(store.read(|session| session.list_jobs(&no_kind)));
    assert_eq!(total, 0);
    assert!(jobs.is_empty());
    Ok(())
}

#[test]
fn runs_come_in_start_order_and_stats_group_by_status() -> TestResult {
    let mut store = store()?;
    put(
        &mut store,
        Job {
            status: JobStatus::Succeeded,
            ..job(1, "a", 100, at(1, 0), at(1, 0))
        },
    );
    put(&mut store, job(2, "a", 100, at(1, 0), at(1, 1)));
    put(
        &mut store,
        Job {
            workspace_id: None,
            ..job(3, "a", 100, at(1, 0), at(1, 2))
        },
    );
    let other_workspace = id(200);
    put(
        &mut store,
        Job {
            workspace_id: Some(other_workspace),
            ..job(4, "a", 100, at(1, 0), at(1, 3))
        },
    );

    for (run, attempt, started) in [(51u32, 1i64, at(0, 30)), (52, 2, at(0, 10))] {
        put(
            &mut store,
            JobRun {
                id: id(run),
                job_id: id(1),
                attempt_number: attempt,
                worker_id: "w1".to_owned(),
                status: "succeeded".to_owned(),
                error: None,
                started_at: started,
                finished_at: None,
            },
        );
    }
    let runs = ok(store.read(|session| session.job_runs(id(1))));
    let attempts: Vec<i64> = runs.iter().map(|run| run.attempt_number).collect();
    assert_eq!(attempts, vec![2, 1]);

    let stats = ok(store.read(|session| session.job_stats(Some(workspace()))));
    assert_eq!(stats.get("succeeded"), Some(&1));
    assert_eq!(stats.get("queued"), Some(&2));
    assert_eq!(stats.len(), 2);

    // Jobs with no workspace count for every workspace; job 4 belongs only to `other_workspace`.
    let stats = ok(store.read(|session| session.job_stats(Some(other_workspace))));
    assert_eq!(stats.get("queued"), Some(&2));
    assert_eq!(stats.get("succeeded"), None);

    let stats = ok(store.read(|session| session.job_stats(None)));
    assert_eq!(stats.get("queued"), Some(&3));
    assert_eq!(stats.get("succeeded"), Some(&1));
    Ok(())
}

#[test]
fn worker_listing_is_newest_first_and_capped() -> TestResult {
    let mut store = store()?;
    for i in 0u8..52 {
        let row = Worker {
            id: format!("w{i:02}"),
            hostname: "host".to_owned(),
            pid: i64::from(i),
            status: "running".to_owned(),
            concurrency: 1,
            kinds: VectorSync::new_sync(),
            current_jobs: VectorSync::new_sync(),
            jobs_completed: 0,
            jobs_failed: 0,
            started_at: at(0, i),
            heartbeat_at: at(0, i),
            stopped_at: None,
        };
        put(&mut store, row);
    }
    let workers = ok(store.read(|session| session.workers()));
    assert_eq!(workers.len(), 50);
    assert_eq!(workers[0].id, "w51");
    assert_eq!(workers[49].id, "w02");
    Ok(())
}

#[test]
fn system_cursor_reads_by_name() -> TestResult {
    let mut store = store()?;
    put(
        &mut store,
        SystemCursor {
            name: "maintenance:x".to_owned(),
            value: 7,
            updated_at: at(0, 1),
        },
    );
    let found =
        ok(store.read(|session| session.system_cursor("maintenance:x"))).ok_or("cursor missing")?;
    assert_eq!(found.value, 7);
    assert!(ok(store.read(|session| session.system_cursor("nope"))).is_none());
    Ok(())
}
