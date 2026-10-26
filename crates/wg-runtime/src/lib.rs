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
mod automation;
mod bus;
mod cli_state;
mod cycle;
mod effects;
mod feature;
mod git;
mod git_sync;
mod handlers;
mod hooks;
mod jobs;
mod logging;
mod queue;
mod stacks;
mod stamp;
mod tasks;
pub use tasks::index_written;
mod worker;
mod worktrees;

pub use app::{App, environment};
pub use bus::Bus;
pub use cli_state::CliState;
pub use cycle::{Flow, Outcome, Runtime, Step};
pub use effects::{HttpClient, Interpreter, NetHttp, write_atomic};
pub use feature::{
    CliGroup, CliRun, Feature, JobKind, JobRun, ResourceRun, RouteGroup, RouteRequest, RouteRun,
    ToolRun,
};
pub use handlers::{Handler, HandlerError, HandlerRegistry, HandlerResult, JobContext};
pub use hooks::{Failure, IngestResult, IngestStatus, ReplayCounts, ingest_direct, spool_record};
pub use logging::{JsonLayer, LogSettings, RotatingFile, configure_logging, json_layer};
pub use queue::Queue;
pub use stamp::{
    StampSource, SystemStamps, agent_context, draw, human_context, new_id, system_context,
};
pub use wg_runtimespec::automation::catalog as automation_catalog;
pub use wg_runtimespec::jobs::{unknown_job_kind, worker_summary_out};
pub use wg_runtimespec::tasks::reads::{GraphRequest, Shown};
pub use wg_runtimespec::{Level, RequestContext};
pub use wg_tasks::rename::Term;
pub use worker::{RunOptions, Summary, Worker, WorkerOptions};
