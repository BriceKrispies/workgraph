//! Job handlers (`jobs/handlers.py`): the context a handler runs in, the registry that maps a job
//! kind to its handler, and the handlers whose services exist in Rust.
//!
//! The Python `HANDLERS` table has 17 kinds. A kind whose handler belongs to a service that is not
//! ported yet has no entry here: the worker fails such a job with a "handler not ported" error
//! instead of pretending it ran. `wg_jobs::dispatch::unwired_kinds` lists them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use wg_jobs::dispatch::{WorkerSettings, is_truthy};
use wg_jobs::queue::ClaimedJob;
use wg_model::entity::JsonObject;
use wg_types::WgError;

use crate::feature::{Feature, JobKind};
use crate::queue::Queue;

/// Why a handler did not finish normally.
#[derive(Debug, Clone, PartialEq)]
pub enum HandlerError {
    /// The handler saw the cancellation request (`JobCancelled`).
    Cancelled(String),
    /// A domain error: its code decides whether the queue retries.
    Failed(WgError),
    /// An unexpected failure (Python's bare exception): retried, kind `exception`.
    Exception(String),
}

/// What a handler returns: the job's result object, if any.
pub type HandlerResult = Result<Option<JsonObject>, HandlerError>;

/// The job a handler runs and the cancellation flag the worker raises for it. Clones share the flag.
#[derive(Debug, Clone)]
pub struct JobContext {
    pub job: ClaimedJob,
    pub worker_id: String,
    cancel: Arc<AtomicBool>,
}

impl JobContext {
    pub fn new(job: ClaimedJob, worker_id: &str) -> JobContext {
        JobContext {
            job,
            worker_id: worker_id.to_owned(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Raises the cancellation flag (a cancel request, a lost lease or a timeout).
    pub fn request_cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancel_requested(&mut self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// `check_cancelled`: an error once cancellation was requested.
    pub fn check_cancelled(&mut self) -> Result<(), HandlerError> {
        if self.cancel_requested() {
            Err(HandlerError::Cancelled(format!(
                "job {} cancelled",
                self.job.id
            )))
        } else {
            Ok(())
        }
    }
}

/// A job handler. It runs outside any database transaction, on the job's own connection.
pub trait Handler: Send + Sync {
    fn run(&self, queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult;
}

impl<F> Handler for F
where
    F: Fn(&mut Queue, &mut JobContext) -> HandlerResult + Send + Sync,
{
    fn run(&self, queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
        self(queue, ctx)
    }
}

/// Job kind to handler.
#[derive(Clone, Default)]
pub struct HandlerRegistry {
    pub handlers: BTreeMap<String, Arc<dyn Handler>>,
}

impl HandlerRegistry {
    /// The handlers this build wires: [`RUNTIME_JOBS`], the runtime's own kinds.
    pub fn standard() -> HandlerRegistry {
        RUNTIME_JOBS
            .iter()
            .fold(HandlerRegistry::default(), |mut registry, job| {
                registry.register(job.kind, job.handler);
                registry
            })
    }

    /// [`HandlerRegistry::standard`] plus the job kinds of `features`, in order; a later feature
    /// replaces an earlier handler of the same kind.
    pub fn from_features(features: &[&Feature]) -> HandlerRegistry {
        features
            .iter()
            .flat_map(|feature| feature.jobs.iter())
            .fold(HandlerRegistry::standard(), |mut registry, job| {
                registry.register(job.kind, job.handler);
                registry
            })
    }

    /// Registers (or replaces) the handler of `kind`.
    pub fn register(&mut self, kind: &str, handler: impl Handler + 'static) {
        self.handlers.insert(kind.to_owned(), Arc::new(handler));
    }
}

/// The job kinds owned by the runtime itself rather than a feature: the `noop` diagnostic and the
/// `compact_telemetry` retention pass. Both stay here until a feature crate owns them (4.16, FC-09),
/// and a move is a cut of its entry and its function into that feature's `jobs` table.
pub static RUNTIME_JOBS: &[JobKind] = &[
    JobKind {
        kind: "noop",
        handler: noop,
    },
    JobKind {
        kind: "compact_telemetry",
        handler: compact_telemetry,
    },
];

/// `noop`: the diagnostic job that checks workers are alive. `sleep` seconds are spent in slices,
/// watching for cancellation; `fail` makes the job raise.
fn noop(_queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let payload = ctx.job.payload.clone();
    let seconds = payload.get("sleep").and_then(Value::as_f64).unwrap_or(0.0);
    let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    while Instant::now() < deadline {
        ctx.check_cancelled()?;
        std::thread::sleep(Duration::from_secs_f64(seconds.min(0.1)));
    }
    if let Some(message) = payload.get("fail").filter(|value| is_truthy(value)) {
        let text = message
            .as_str()
            .map_or_else(|| message.to_string(), str::to_owned);
        return Err(HandlerError::Exception(format!("RuntimeError: {text}")));
    }
    let mut result = JsonObject::new();
    result.insert("ok".to_owned(), Value::Bool(true));
    result.insert("attempt".to_owned(), Value::from(ctx.job.attempt));
    Ok(Some(result))
}

/// `compact_telemetry`: the retention pass; `dry_run` reports without changing anything.
fn compact_telemetry(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let config = queue.app.config().map_err(HandlerError::Failed)?;
    let settings = WorkerSettings::from_config(&config);
    let dry_run = ctx.job.payload.get("dry_run").is_some_and(is_truthy);
    queue
        .compact_telemetry(&settings, dry_run)
        .map(Some)
        .map_err(HandlerError::Failed)
}
