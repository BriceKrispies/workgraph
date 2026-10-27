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
pub use acceptance::{integration_busy, latest_succeeded};
mod app;
mod bus;
mod cli_state;
mod cycle;
mod effects;
mod feature;
mod handlers;
mod jobs;
mod logging;
mod queue;
mod scope;
mod spool;
mod stacks;
mod stamp;
pub use scope::index_written;
mod worker;

pub use app::{App, environment, resolve_paths};
pub use bus::Bus;
pub use cli_state::CliState;
pub use cycle::{Flow, Outcome, Runtime, Step};
pub use effects::{HttpClient, Interpreter, NetHttp, write_atomic};
pub use feature::{
    CliGroup, CliRun, Feature, JobKind, JobRun, MaintenanceRun, MaintenanceStep, ResourceRun,
    RouteGroup, RouteRequest, RouteRun, ToolRun,
};
pub use handlers::{Handler, HandlerError, HandlerRegistry, HandlerResult, JobContext};
pub use logging::{JsonLayer, LogSettings, RotatingFile, configure_logging, json_layer};
pub use queue::Queue;
pub use spool::{spool_record, write_spooled};
pub use stamp::{
    StampSource, SystemStamps, agent_context, draw, human_context, new_id, system_context,
};
pub use wg_runtimespec::jobs::{unknown_job_kind, worker_summary_out};
pub use wg_runtimespec::{Level, RequestContext};
pub use wg_tasks::rename::Term;
pub use worker::{RunOptions, Summary, Worker, WorkerOptions};
