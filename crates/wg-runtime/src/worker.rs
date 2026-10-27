//! The background worker (`jobs/worker.py`): claims jobs from the durable queue, runs their
//! handlers on threads, keeps their leases alive and performs maintenance when elected leader.
//!
//! * Each running job has a heartbeat thread that extends its lease and relays a cancellation
//!   request (or a timeout) to the handler's [`JobContext`]. A crashed worker simply stops
//!   heartbeating; another worker recovers its jobs once the lease expires.
//! * One worker at a time performs the periodic maintenance, elected through the
//!   `system:maintenance` lease.
//! * Shutdown is graceful: no new jobs are claimed and running jobs get a grace period.
//!
//! Every thread opens its own [`Queue`] (its own database connection). The decisions (retry or
//! not, heartbeat cadence, settings, the maintenance plan) are `wg-jobs`'; the types here hold the
//! threads and the loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use wg_jobs::dispatch::{
    DEFAULT_GRACE_SECONDS, DEFAULT_MAINTENANCE_SECONDS, DEFAULT_POLL_SECONDS, ERROR_KIND_EXCEPTION,
    WorkerSettings, default_worker_id, failure_for, heartbeat_interval_seconds,
    maintenance_lease_seconds, not_ported, timed_out,
};
use wg_jobs::queue::ClaimedJob;
use wg_jobs::workers::NewWorker;
use wg_types::{ErrorCode, Id, WgError};

use crate::handlers::{HandlerError, HandlerRegistry, JobContext};
use crate::queue::Queue;
use crate::stamp::draw;

/// How a worker is set up (`workgraph worker run`'s options).
#[derive(Debug, Clone, Default)]
pub struct WorkerOptions {
    /// The Workgraph home; `None` is the default home.
    pub home: Option<PathBuf>,
    /// Concurrent jobs; `None` is `jobs.worker_concurrency`.
    pub concurrency: Option<usize>,
    /// The job kinds to claim; empty claims every kind.
    pub kinds: Vec<String>,
    /// Idle poll; `None` is one second.
    pub poll_interval: Option<Duration>,
    /// Between maintenance passes; `None` is five seconds.
    pub maintenance_interval: Option<Duration>,
    pub worker_id: Option<String>,
}

/// How long [`Worker::run`] goes on.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Drain the runnable jobs and return.
    pub once: bool,
    /// Return after this long with nothing to do.
    pub max_idle: Option<Duration>,
    /// How long running jobs get on shutdown; `None` is thirty seconds.
    pub grace: Option<Duration>,
}

/// What a finished worker did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub completed: i64,
    pub failed: i64,
}

/// A job on its thread.
struct Running {
    job_id: Id,
    handle: JoinHandle<()>,
}

/// A worker: the queue it claims from, its threads and its counters.
pub struct Worker {
    queue: Queue,
    options: WorkerOptions,
    settings: WorkerSettings,
    /// Concurrent jobs this worker runs.
    pub concurrency: usize,
    pub worker_id: String,
    registry: Arc<HandlerRegistry>,
    stop: Arc<AtomicBool>,
    running: Vec<Running>,
    completed: Arc<AtomicI64>,
    failed: Arc<AtomicI64>,
    last_maintenance: Option<Instant>,
    last_heartbeat: Option<Instant>,
}

/// Seconds between the worker's own heartbeats.
const WORKER_HEARTBEAT: Duration = Duration::from_secs(5);

impl Worker {
    /// Opens the application and reads the settings.
    pub fn open(options: WorkerOptions, registry: HandlerRegistry) -> Result<Worker, WgError> {
        let queue = Queue::open(options.home.as_deref(), "worker")?;
        let settings = WorkerSettings::from_config(&queue.app.config()?);
        let concurrency = options
            .concurrency
            .unwrap_or_else(|| usize::try_from(settings.concurrency).unwrap_or(1))
            .max(1);
        let worker_id = match options.worker_id.clone() {
            Some(id) => id,
            None => {
                let (_, entropy) = draw()?;
                default_worker_id(&hostname(), std::process::id(), &entropy)
            }
        };
        Ok(Worker {
            queue,
            options,
            settings,
            concurrency,
            worker_id,
            registry: Arc::new(registry),
            stop: Arc::new(AtomicBool::new(false)),
            running: Vec::new(),
            completed: Arc::new(AtomicI64::new(0)),
            failed: Arc::new(AtomicI64::new(0)),
            last_maintenance: None,
            last_heartbeat: None,
        })
    }

    /// A flag that stops the loop when set (from a signal handler or another thread).
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// The queue, for the caller's own enqueues and reads.
    pub fn queue(&mut self) -> &mut Queue {
        &mut self.queue
    }

    fn summary(&mut self) -> Summary {
        Summary {
            completed: self.completed.load(Ordering::SeqCst),
            failed: self.failed.load(Ordering::SeqCst),
        }
    }

    fn register(&mut self) -> Result<(), WgError> {
        let worker = NewWorker {
            id: self.worker_id.clone(),
            hostname: hostname(),
            pid: i64::from(std::process::id()),
            concurrency: i64::try_from(self.concurrency).unwrap_or(i64::MAX),
            kinds: self.options.kinds.clone(),
        };
        self.queue.register_worker(&worker)
    }

    /// Recovery problems must not stop a worker from starting.
    fn startup_recovery(&mut self) {
        if let Err(error) = self.queue.recover_expired() {
            tracing::error!(target: "workgraph.worker", error = %error.message, "startup recovery failed");
        }
    }

    /// Runs until stopped. `once` drains the runnable jobs and returns; `max_idle` returns after
    /// that long with nothing to do.
    pub fn run(&mut self, run: RunOptions) -> Result<Summary, WgError> {
        self.register()?;
        tracing::info!(target: "workgraph.worker", worker = %self.worker_id, concurrency = self.concurrency, "worker started");
        self.startup_recovery();
        let mut idle_since = Instant::now();
        while !self.stop.load(Ordering::SeqCst) {
            self.maybe_maintenance(false);
            self.reap();
            let seen = self.queue.app.runtime.effects.bus.wakes();
            let claimed_any = self.fill();
            if claimed_any || !self.running.is_empty() {
                idle_since = Instant::now();
            }
            if run.once && self.running.is_empty() && !claimed_any {
                break;
            }
            if run
                .max_idle
                .is_some_and(|limit| idle_since.elapsed() > limit)
            {
                break;
            }
            self.heartbeat_worker();
            let wait = if claimed_any {
                Duration::from_millis(50)
            } else {
                self.options
                    .poll_interval
                    .unwrap_or(Duration::from_secs_f64(DEFAULT_POLL_SECONDS))
            };
            self.queue.app.runtime.effects.bus.wait_for_wake(seen, wait);
        }
        self.shutdown(
            run.grace
                .unwrap_or(Duration::from_secs_f64(DEFAULT_GRACE_SECONDS)),
        );
        Ok(self.summary())
    }

    /// Processes jobs until the queue has nothing runnable (tests and demos).
    pub fn run_until_idle(&mut self, timeout: Duration) -> Result<Summary, WgError> {
        self.register()?;
        let deadline = Instant::now() + timeout;
        let mut drained = false;
        while Instant::now() < deadline {
            self.maybe_maintenance(true);
            self.reap();
            let started = self.fill();
            if !started && self.running.is_empty() {
                self.maybe_maintenance(true);
                if !self.queue.claim_peek(&self.options.kinds)? {
                    drained = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.shutdown(Duration::from_secs_f64(DEFAULT_GRACE_SECONDS));
        if drained {
            Ok(self.summary())
        } else {
            Err(WgError::workgraph("jobs did not drain before the timeout"))
        }
    }

    /// Claims jobs while there is a free thread; whether any was claimed.
    fn fill(&mut self) -> bool {
        let mut claimed_any = false;
        while self.running.len() < self.concurrency && !self.stop.load(Ordering::SeqCst) {
            let claim = self.queue.claim(
                &self.worker_id,
                &self.options.kinds,
                self.settings.lease_seconds,
                &self.settings.limits,
            );
            match claim {
                Ok(Some(job)) => {
                    claimed_any = true;
                    self.start(job);
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::error!(target: "workgraph.worker", error = %error.message, "claim failed");
                    break;
                }
            }
        }
        claimed_any
    }

    /// Starts the job on its own thread.
    fn start(&mut self, job: ClaimedJob) {
        let job_id = job.id;
        let runner = JobRunner {
            home: self.options.home.clone(),
            ctx: JobContext::new(job, &self.worker_id),
            registry: Arc::clone(&self.registry),
            lease_seconds: self.settings.lease_seconds,
            completed: Arc::clone(&self.completed),
            failed: Arc::clone(&self.failed),
        };
        match std::thread::Builder::new()
            .name(format!(
                "wg-job-{}",
                job_id.to_string().chars().take(8).collect::<String>()
            ))
            .spawn(move || runner.run())
        {
            Ok(handle) => self.running.push(Running { job_id, handle }),
            Err(error) => {
                tracing::error!(target: "workgraph.worker", job = %job_id, error = %error, "cannot start job thread");
            }
        }
    }

    /// Forgets the jobs whose threads finished.
    fn reap(&mut self) {
        self.running.retain(|running| !running.handle.is_finished());
    }

    /// Waits up to `grace` for the running jobs, then stops the worker row.
    fn shutdown(&mut self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while !self.running.is_empty() && Instant::now() < deadline {
            self.reap();
            std::thread::sleep(Duration::from_millis(100));
        }
        self.running.clear();
        // The counters the worker row shows are the ones it last wrote: write the final ones.
        let Summary { completed, failed } = self.summary();
        if let Err(error) = self
            .queue
            .worker_heartbeat(&self.worker_id, &[], completed, failed)
        {
            tracing::error!(target: "workgraph.worker", error = %error.message, "worker heartbeat failed");
        }
        if let Err(error) = self.queue.stop_worker(&self.worker_id) {
            tracing::error!(target: "workgraph.worker", error = %error.message, "cannot stop worker row");
        }
        tracing::info!(
            target: "workgraph.worker",
            worker = %self.worker_id,
            completed = self.completed.load(Ordering::SeqCst),
            failed = self.failed.load(Ordering::SeqCst),
            "worker stopped"
        );
    }

    fn heartbeat_worker(&mut self) {
        if self
            .last_heartbeat
            .is_some_and(|at| at.elapsed() < WORKER_HEARTBEAT)
        {
            return;
        }
        self.last_heartbeat = Some(Instant::now());
        let current: Vec<String> = self.running.iter().map(|r| r.job_id.to_string()).collect();
        let Summary { completed, failed } = self.summary();
        if let Err(error) =
            self.queue
                .worker_heartbeat(&self.worker_id, &current, completed, failed)
        {
            tracing::error!(target: "workgraph.worker", error = %error.message, "worker heartbeat failed");
        }
    }

    /// Runs the maintenance pass when it is due (or `force`); failures are logged, never raised.
    pub fn maybe_maintenance(&mut self, force: bool) -> Option<Map<String, Value>> {
        let interval = self
            .options
            .maintenance_interval
            .unwrap_or(Duration::from_secs_f64(DEFAULT_MAINTENANCE_SECONDS));
        if !force
            && self
                .last_maintenance
                .is_some_and(|at| at.elapsed() < interval)
        {
            return None;
        }
        self.last_maintenance = Some(Instant::now());
        let lease = maintenance_lease_seconds(interval.as_secs_f64());
        let steps = &self.registry.maintenance;
        match self
            .queue
            .maintain(&self.worker_id, &self.settings, lease, steps)
        {
            Ok(report) => Some(report),
            Err(error) => {
                tracing::error!(target: "workgraph.worker", error = %error.message, "maintenance failed");
                None
            }
        }
    }
}

/// The host name (`platform.node()`), from the environment.
fn hostname() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "localhost".to_owned())
}

/// One claimed job on its thread: its own queue connection, a heartbeat thread, the handler and
/// the recording of its outcome.
struct JobRunner {
    home: Option<PathBuf>,
    ctx: JobContext,
    registry: Arc<HandlerRegistry>,
    lease_seconds: i64,
    completed: Arc<AtomicI64>,
    failed: Arc<AtomicI64>,
}

/// What a job's handler ended in.
enum Ended {
    Done(Option<wg_model::entity::JsonObject>),
    Error(HandlerError),
    Panicked(String),
    NoHandler,
}

impl JobRunner {
    fn run(mut self) {
        let mut queue = match Queue::open(self.home.as_deref(), "worker") {
            Ok(queue) => queue,
            Err(error) => {
                // The lease runs out and another worker recovers the job.
                tracing::error!(target: "workgraph.worker", job = %self.ctx.job.id, error = %error.message, "cannot open the job's connection");
                return;
            }
        };
        let (stop, stopped) = channel::<()>();
        let beat = self.spawn_heartbeat(stopped);
        let ended = self.handle(&mut queue);
        drop(stop);
        if let Some(beat) = beat {
            let _ = beat.join();
        }
        self.record(&mut queue, ended);
    }

    /// The heartbeat thread: extends the lease, relays cancellation, enforces the timeout.
    fn spawn_heartbeat(&self, stopped: Receiver<()>) -> Option<JoinHandle<()>> {
        let beat = Heartbeat {
            home: self.home.clone(),
            ctx: self.ctx.clone(),
            lease_seconds: self.lease_seconds,
            started: Instant::now(),
        };
        std::thread::Builder::new()
            .name("wg-heartbeat".to_owned())
            .spawn(move || beat.run(&stopped))
            .ok()
    }

    fn handle(&mut self, queue: &mut Queue) -> Ended {
        let Some(handler) = self.registry.handlers.get(&self.ctx.job.kind).cloned() else {
            return Ended::NoHandler;
        };
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler.run(queue, &mut self.ctx)
        }));
        match run {
            Ok(Ok(result)) => Ended::Done(result),
            Ok(Err(error)) => Ended::Error(error),
            Err(panic) => Ended::Panicked(
                panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "handler panicked".to_owned()),
            ),
        }
    }

    /// Records the outcome on the queue; a lost lease only gets logged.
    fn record(&mut self, queue: &mut Queue, ended: Ended) {
        let job = self.ctx.job.id;
        let worker = self.ctx.worker_id.clone();
        let worker = worker.as_str();
        let recorded = match ended {
            Ended::Done(_) if self.ctx.cancel_requested() => {
                queue.mark_cancelled(job, worker, "cancellation requested")
            }
            Ended::Done(result) => {
                self.completed.fetch_add(1, Ordering::SeqCst);
                queue.complete(job, worker, result)
            }
            Ended::Error(HandlerError::Cancelled(reason)) => {
                queue.mark_cancelled(job, worker, &reason)
            }
            Ended::Error(HandlerError::Failed(error)) => {
                self.failed.fetch_add(1, Ordering::SeqCst);
                let failure = failure_for(error.code);
                queue
                    .fail(
                        job,
                        worker,
                        &error.message,
                        failure.retryable,
                        &failure.error_kind,
                    )
                    .map(|_| ())
            }
            Ended::Error(HandlerError::Exception(text)) | Ended::Panicked(text) => {
                self.failed.fetch_add(1, Ordering::SeqCst);
                tracing::error!(
                    target: "workgraph.worker",
                    job = %job,
                    kind = %self.ctx.job.kind,
                    exc = %text,
                    "job failed"
                );
                queue
                    .fail(job, worker, &text, true, ERROR_KIND_EXCEPTION)
                    .map(|_| ())
            }
            Ended::NoHandler => {
                self.failed.fetch_add(1, Ordering::SeqCst);
                let (failure, message) = not_ported(&self.ctx.job.kind);
                queue
                    .fail(
                        job,
                        worker,
                        &message,
                        failure.retryable,
                        &failure.error_kind,
                    )
                    .map(|_| ())
            }
        };
        if let Err(error) = recorded {
            tracing::error!(target: "workgraph.worker", job = %job, error = %error.message, "cannot record the job's outcome");
        }
    }
}

/// The heartbeat of one running job.
struct Heartbeat {
    home: Option<PathBuf>,
    ctx: JobContext,
    lease_seconds: i64,
    started: Instant,
}

impl Heartbeat {
    /// Beats until `stopped` closes. A conflict (the lease is gone) cancels the handler and ends.
    fn run(mut self, stopped: &Receiver<()>) {
        let Ok(mut queue) = Queue::open(self.home.as_deref(), "worker") else {
            return;
        };
        let interval = Duration::from_secs_f64(heartbeat_interval_seconds(self.lease_seconds));
        while matches!(
            stopped.recv_timeout(interval),
            Err(RecvTimeoutError::Timeout)
        ) {
            match queue.heartbeat(self.ctx.job.id, &self.ctx.worker_id, self.lease_seconds) {
                Ok(true) => self.ctx.request_cancel(),
                Ok(false) => {}
                Err(error) if error.code == ErrorCode::Conflict => {
                    self.ctx.request_cancel();
                    return;
                }
                Err(error) => {
                    tracing::error!(target: "workgraph.worker", job = %self.ctx.job.id, error = %error.message, "heartbeat failed");
                }
            }
            if timed_out(
                self.ctx.job.timeout_seconds,
                self.started.elapsed().as_secs_f64(),
            ) {
                self.ctx.request_cancel();
            }
        }
    }
}
