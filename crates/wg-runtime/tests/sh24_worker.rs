//! wg-runtime worker checks (SH-24) against real SQLite files and real threads: the claim and
//! dispatch loop, the handler registry, lease recovery, cancellation, the maintenance leader, tracker
//! scheduling and telemetry retention.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wg_jobs::dispatch::{WorkerSettings, unwired_kinds};
use wg_jobs::queue::NewJob;
use wg_model::Write;
use wg_model::entity::{Artifact, JsonObject, Worker as WorkerRow, Workspace};
use wg_runtime::{HandlerRegistry, Queue, RunOptions, Worker, WorkerOptions};
use wg_types::{Id, JobStatus, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sh24")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A Workgraph home and a queue on it; opening the app migrates the database to head.
fn home(name: &str) -> Result<(PathBuf, Queue), Box<dyn Error>> {
    let dir = scratch(name)?;
    let queue = Queue::open(Some(&dir), "test")?;
    Ok((dir, queue))
}

fn write_config(queue: &Queue, text: &str) -> TestResult {
    std::fs::write(queue.app.paths.config_file().as_str(), text)?;
    Ok(())
}

fn options(dir: &Path) -> WorkerOptions {
    WorkerOptions {
        home: Some(dir.to_path_buf()),
        poll_interval: Some(Duration::from_millis(50)),
        maintenance_interval: Some(Duration::from_millis(100)),
        ..WorkerOptions::default()
    }
}

fn job_row(queue: &mut Queue, id: Id) -> Result<wg_model::entity::Job, Box<dyn Error>> {
    queue
        .app
        .runtime
        .store
        .read(|session| session.job(id))?
        .ok_or_else(|| "job not found".into())
}

fn payload(value: Value) -> JsonObject {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

#[test]
fn registry_lists_wired_and_unwired_kinds() {
    let registry = HandlerRegistry::standard();
    let wired: Vec<String> = registry.handlers.keys().cloned().collect();
    assert_eq!(wired, ["compact_telemetry", "noop"]);
    let unwired = unwired_kinds(&wired);
    assert_eq!(unwired.len(), 15);
    assert!(unwired.contains(&"git_sync") && unwired.contains(&"agent_run"));
}

#[test]
fn a_noop_job_runs_to_success() -> TestResult {
    let (dir, mut queue) = home("noop")?;
    let id = queue.enqueue(&NewJob {
        payload: payload(json!({"sleep": 0.05})),
        ..NewJob::new("noop")
    })?;
    let mut worker = Worker::open(options(&dir), HandlerRegistry::standard())?;
    let summary = worker.run_until_idle(Duration::from_secs(30))?;
    assert_eq!((summary.completed, summary.failed), (1, 0));
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(job.result, Some(payload(json!({"ok": true, "attempt": 1}))));
    // The worker row was written and then stopped.
    let workers = queue.app.runtime.store.read(|session| session.workers())?;
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].status, "stopped");
    Ok(())
}

#[test]
fn an_unported_kind_fails_the_job_without_retry() -> TestResult {
    let (dir, mut queue) = home("unported")?;
    let id = queue.enqueue(&NewJob::new("git_sync"))?;
    let mut worker = Worker::open(options(&dir), HandlerRegistry::standard())?;
    let summary = worker.run_until_idle(Duration::from_secs(30))?;
    assert_eq!((summary.completed, summary.failed), (0, 1));
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.attempts, 1);
    assert_eq!(job.error_kind.as_deref(), Some("handler_not_ported"));
    assert_eq!(
        job.last_error.as_deref(),
        Some("handler not ported for job kind 'git_sync'")
    );
    Ok(())
}

#[test]
fn a_raising_handler_is_retried_then_dead_lettered() -> TestResult {
    let (dir, mut queue) = home("raising")?;
    let id = queue.enqueue(&NewJob {
        payload: payload(json!({"fail": "boom"})),
        max_attempts: 2,
        backoff_base_seconds: 0.0,
        ..NewJob::new("noop")
    })?;
    let mut worker = Worker::open(options(&dir), HandlerRegistry::standard())?;
    let summary = worker.run_until_idle(Duration::from_secs(30))?;
    assert_eq!(summary.failed, 2);
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.attempts, 2);
    assert_eq!(job.error_kind.as_deref(), Some("exception"));
    assert_eq!(job.last_error.as_deref(), Some("RuntimeError: boom"));
    Ok(())
}

#[test]
fn a_panicking_handler_fails_the_job_and_the_worker_survives() -> TestResult {
    let (dir, mut queue) = home("panic")?;
    let id = queue.enqueue(&NewJob {
        max_attempts: 1,
        ..NewJob::new("noop")
    })?;
    let mut registry = HandlerRegistry::standard();
    registry.register(
        "noop",
        |_queue: &mut Queue, _ctx: &mut wg_runtime::JobContext| -> wg_runtime::HandlerResult {
            panic!("handler exploded")
        },
    );
    let mut worker = Worker::open(options(&dir), registry)?;
    worker.run_until_idle(Duration::from_secs(30))?;
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.error_kind.as_deref(), Some("exception"));
    assert_eq!(job.last_error.as_deref(), Some("handler exploded"));
    Ok(())
}

#[test]
fn recover_requeues_expired_leases_and_marks_lost_workers() -> TestResult {
    let (_dir, mut queue) = home("recover")?;
    let id = queue.enqueue(&NewJob::new("noop"))?;
    let limits = WorkerSettings::from_config(&queue.app.config()?).limits;
    // A worker claims with a zero-second lease and never heartbeats.
    let claimed = queue.claim("ghost", &[], 0, &limits)?;
    assert_eq!(claimed.map(|job| job.id), Some(id));
    assert_eq!(job_row(&mut queue, id)?.status, JobStatus::Running);
    // A running worker whose last heartbeat is years old.
    let stale = Timestamp::from_components(2020, 1, 1, 0, 0, 0, 0)?;
    queue.app.runtime.decide(
        &wg_runtime::system_context("test")?,
        |_| Ok(()),
        |(), _stamp| {
            Ok(wg_model::Decision::new(()).write(Write::insert(WorkerRow {
                id: "old-worker".to_owned(),
                hostname: "h".to_owned(),
                pid: 1,
                status: "running".to_owned(),
                concurrency: 1,
                kinds: Default::default(),
                current_jobs: Default::default(),
                jobs_completed: 0,
                jobs_failed: 0,
                started_at: stale,
                heartbeat_at: stale,
                stopped_at: None,
            })))
        },
    )?;
    std::thread::sleep(Duration::from_millis(20));
    let report = queue.recover()?;
    assert_eq!(report["recovered_jobs"], json!([id.to_string()]));
    assert_eq!(report["lost_workers"], json!(["old-worker"]));
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.error_kind.as_deref(), Some("lease_expired"));
    let workers = queue.app.runtime.store.read(|session| session.workers())?;
    assert_eq!(workers[0].status, "lost");
    Ok(())
}

#[test]
fn cancelling_a_running_job_reaches_the_handler_through_the_heartbeat() -> TestResult {
    let (dir, mut queue) = home("cancel")?;
    // The smallest lease, so the heartbeat runs every couple of seconds.
    write_config(&queue, "[jobs]\nlease_seconds = 5\n")?;
    let id = queue.enqueue(&NewJob {
        payload: payload(json!({"sleep": 60})),
        ..NewJob::new("noop")
    })?;
    let mut worker = Worker::open(options(&dir), HandlerRegistry::standard())?;
    let stop = worker.stop_flag();
    let handle = std::thread::spawn(move || worker.run(RunOptions::default()));
    let deadline = Instant::now() + Duration::from_secs(30);
    while job_row(&mut queue, id)?.status != JobStatus::Running {
        assert!(Instant::now() < deadline, "the job never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(queue.cancel(id)?, JobStatus::Running);
    while job_row(&mut queue, id)?.status == JobStatus::Running {
        assert!(
            Instant::now() < deadline,
            "the job never saw the cancellation"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let summary = handle.join().map_err(|_| "worker thread panicked")??;
    assert_eq!(summary.failed, 0);
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Cancelled);
    Ok(())
}

#[test]
fn one_worker_leads_maintenance_and_queues_tracker_syncs_once() -> TestResult {
    let (_dir, mut queue) = home("maintenance")?;
    write_config(
        &queue,
        "[trackers.jira1]\nbase_url = \"https://jira.example\"\nworkspace = \"default\"\nprojects = [\"WG\"]\ntoken_env = \"JIRA_TOKEN\"\n",
    )?;
    let settings = WorkerSettings::from_config(&queue.app.config()?);
    assert_eq!(settings.trackers.len(), 1);

    let first = queue.maintain("worker-a", &settings, 30)?;
    assert_eq!(first["leader"], json!(true));
    assert_eq!(
        first["tracker_syncs"],
        json!({"incremental": 1, "full": 1, "failed": 0})
    );
    // The retention window opened for the first time; the services not ported are named.
    assert_eq!(first["automation"], json!("not ported"));
    // Another worker does not lead while the lease is held.
    let second = queue.maintain("worker-b", &settings, 30)?;
    assert_eq!(second["leader"], json!(false));
    // The leader's next pass finds the cadences not yet elapsed: nothing more is queued.
    let third = queue.maintain("worker-a", &settings, 30)?;
    assert_eq!(
        third["tracker_syncs"],
        json!({"incremental": 0, "full": 0, "failed": 0})
    );
    let stats = queue
        .app
        .runtime
        .store
        .read(|session| session.job_stats(None))?;
    assert_eq!(stats.get("queued"), Some(&2));
    Ok(())
}

#[test]
fn retention_purges_old_telemetry_artifacts_and_their_files() -> TestResult {
    let (_dir, mut queue) = home("retention")?;
    let settings = WorkerSettings::from_config(&queue.app.config()?);
    let root = PathBuf::from(queue.app.paths.artifacts_dir().as_str());
    std::fs::create_dir_all(root.join("logs"))?;
    std::fs::write(root.join("logs/old.log"), b"old telemetry")?;
    std::fs::write(root.join("logs/new.log"), b"new telemetry")?;
    let old = Timestamp::from_components(2020, 1, 1, 0, 0, 0, 0)?;
    let context = wg_runtime::system_context("test")?;
    queue.app.runtime.decide(
        &context,
        |_| Ok(()),
        |(), stamp| {
            let workspace_id = stamp.id(0);
            let artifact = |n: u32, name: &str, created_at: Timestamp| Artifact {
                id: stamp.id(n),
                workspace_id,
                task_id: None,
                attempt_id: None,
                kind: "log".to_owned(),
                name: name.to_owned(),
                media_type: "text/plain".to_owned(),
                path: format!("logs/{name}"),
                size_bytes: 13,
                sha256: "0".repeat(64),
                retention: "telemetry".to_owned(),
                purged_at: None,
                metadata: JsonObject::new(),
                created_at,
            };
            Ok(wg_model::Decision::<()>::new(())
                .write(Write::insert(Workspace {
                    id: workspace_id,
                    slug: "ws".to_owned(),
                    name: "WS".to_owned(),
                    description: String::new(),
                    key_prefix: "WG".to_owned(),
                    next_task_number: 1,
                    settings: Default::default(),
                    archived_at: None,
                    created_at: stamp.at,
                    updated_at: stamp.at,
                }))
                .write(Write::insert(artifact(1, "old.log", old)))
                .write(Write::insert(artifact(2, "new.log", stamp.at))))
        },
    )?;

    // A dry run reports and changes nothing.
    let dry = queue.compact_telemetry(&settings, true)?;
    assert_eq!(dry["artifacts"], json!(1));
    assert_eq!(dry["bytes_freed"], json!(13));
    assert!(root.join("logs/old.log").exists());

    let report = queue.compact_telemetry(&settings, false)?;
    assert_eq!(report["artifacts"], json!(1));
    assert_eq!(report["dry_run"], json!(false));
    assert!(!root.join("logs/old.log").exists());
    assert!(root.join("logs/new.log").exists());
    let purged = queue.app.runtime.store.read(|session| {
        session.purgeable_artifacts(
            "telemetry",
            Timestamp::from_components(2999, 1, 1, 0, 0, 0, 0)?,
        )
    })?;
    assert_eq!(purged.len(), 1);
    assert_eq!(purged[0].name, "new.log");
    Ok(())
}

#[test]
fn the_compact_telemetry_job_runs_the_retention_pass() -> TestResult {
    let (dir, mut queue) = home("compact_job")?;
    let id = queue.enqueue(&NewJob {
        payload: payload(json!({"dry_run": true})),
        ..NewJob::new("compact_telemetry")
    })?;
    let mut worker = Worker::open(options(&dir), HandlerRegistry::standard())?;
    worker.run_until_idle(Duration::from_secs(30))?;
    let job = job_row(&mut queue, id)?;
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(
        job.result,
        Some(payload(json!({
            "hook_payloads": 0, "artifacts": 0, "bytes_freed": 0, "dry_run": true
        })))
    );
    Ok(())
}
