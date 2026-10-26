//! wg-runtime: the effect interpreter, stamping, the load-decide-apply cycle, flow drivers, JSON
//! logging and the application context (docs/rust-rewrite.md sections 4.1 to 4.5 and 4.13).
//!
//! The runtime is the only loop of the system. An interface (CLI, HTTP, MCP) or a worker opens an
//! [`App`], builds a [`RequestContext`] for the caller, and asks its [`Runtime`] to run a
//! decision: the store loads the slice inside a `BEGIN IMMEDIATE` transaction, a [`StampSource`]
//! stamps the decision, the pure core decides, the store applies the writes and events and
//! commits, and the [`Interpreter`] performs the effects. Long-running work (git sync, runs, stack
//! integration, verification) is a [`Flow`]: a `Core` state machine the runtime drives, feeding
//! each effect's outcome back as the next input.
//!
//! The pure half (request contexts, effect specifications and outcomes, log lines, the app
//! context's rules) is `wg-runtimespec`; this crate keeps the I/O.

mod acceptance;
mod app;
mod bus;
mod cycle;
mod decisions;
mod effects;
mod handlers;
mod hooks;
mod jobs;
mod logging;
mod queue;
mod registry;
mod setup;
mod stacks;
mod stamp;
mod tasks;
mod worker;

pub use app::{App, environment};
pub use bus::Bus;
pub use cycle::{Flow, Outcome, Runtime, Step};
pub use effects::{HttpClient, Interpreter, write_atomic};
pub use handlers::{Handler, HandlerError, HandlerRegistry, HandlerResult, JobContext};
pub use hooks::{Failure, IngestResult, IngestStatus, ReplayCounts, ingest_direct, spool_record};
pub use logging::{JsonLayer, LogSettings, RotatingFile, configure_logging, json_layer};
pub use queue::Queue;
pub use setup::Diagnosis;
pub use stamp::{
    StampSource, SystemStamps, agent_context, draw, human_context, new_id, system_context,
};
pub use wg_runtimespec::decisions::{DecisionFilter, filter_of as decision_filter};
pub use wg_runtimespec::jobs::{unknown_job_kind, worker_summary_out};
pub use wg_runtimespec::registry::{
    project_create_body, project_option_body, workspace_create_body,
};
pub use wg_runtimespec::setup::InitRequest;
pub use wg_runtimespec::tasks::reads::{GraphRequest, Shown};
pub use wg_runtimespec::{Level, RequestContext};
pub use wg_tasks::rename::Term;
pub use worker::{RunOptions, Summary, Worker, WorkerOptions};
