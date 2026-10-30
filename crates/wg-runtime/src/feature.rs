//! The feature descriptor (docs/rust-rewrite.md 4.16, D15): what one vertical feature contributes
//! to each front end and to the worker, as tables of `fn` pointers.
//!
//! A feature is one value of [`Feature`]. The front ends (`wg-cli`, `wg-server`, `wg-mcp`) and the
//! worker collect from an ordered `&[&Feature]`; none of them is named here, so a feature depends
//! on this crate and never on a front end. The handler types use only base and spec types, plus
//! [`crate::CliState`] and [`RouteRequest`], which the runtime owns.

use serde_json::{Map, Value as Json};
use wg_iface::cli::CliFailure;
use wg_iface::cli::{ArgMatches, Command};
use wg_iface::http::groups::RouteSpec;
use wg_iface::mcp::options::Agent;
use wg_jobs::maintenance::Step;
use wg_json::Value;
use wg_types::WgError;

use crate::handlers::{HandlerResult, JobContext};
use crate::queue::Queue;
use crate::{App, CliState, RequestContext};

/// What a CLI group runs for the matched leaf command: the path below the root and the matches.
pub type CliRun = fn(&mut CliState, &[String], &ArgMatches) -> Result<(), CliFailure>;

/// One top-level command or group of the `workgraph` binary.
pub struct CliGroup {
    /// The name as typed after `workgraph`.
    pub name: &'static str,
    /// Builds the clap command; start from `wg_iface::cli::tree::command` so the help flag matches.
    pub command: fn() -> Command,
    /// Runs the matched leaf command of this group.
    pub run: CliRun,
}

/// The request a route handler receives, after the security screen and validation: the path and
/// query parameters by name, the validated body, the body as sent, and the caller's context.
pub struct RouteRequest {
    /// The path and query parameters, validated, by name (defaults filled in).
    pub params: Map<String, Json>,
    /// The validated body, `null` when the route takes none or it was absent.
    pub body: Json,
    /// The body as the client sent it (`null` when absent).
    pub given: Json,
    /// The caller's context (`deps.get_ctx`).
    pub context: RequestContext,
}

/// A file a route answers with instead of a JSON document: its bytes, the media type and the name
/// the client saves it under (`Content-Disposition: attachment`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFile {
    pub content_type: String,
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// What a route operation answers: a JSON document, or a file.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteAnswer {
    Json(Value),
    File(RouteFile),
}

/// What a route group runs for one operation: the application context, the operation's inventory
/// id and the request, in; the document or the file, out.
pub type RouteRun = fn(&mut App, &str, RouteRequest) -> Result<RouteAnswer, WgError>;

/// The operations of one route group and the function that runs them.
pub struct RouteGroup {
    /// The operations, in registration order (`wg_iface::http::groups` tables).
    pub specs: &'static [RouteSpec],
    pub run: RouteRun,
}

/// A tool call the feature answers: `None` when the tool is not one of its own. The arguments are
/// validated against the snapshot descriptors before the call.
pub type ToolRun = fn(&mut App, &Agent, &str, &Json) -> Option<Result<Json, WgError>>;

/// A resource read the feature answers: `None` when the URI is not one of its own.
pub type ResourceRun = fn(&mut App, &Agent, &str) -> Option<Result<String, WgError>>;

/// A job handler function, as the worker runs it.
pub type JobRun = fn(&mut Queue, &mut JobContext) -> HandlerResult;

/// A job kind the feature's handler runs.
pub struct JobKind {
    pub kind: &'static str,
    pub handler: JobRun,
}

/// A maintenance step a feature runs, as the maintenance leader's pass does (`jobs/maintenance.py`).
pub type MaintenanceRun = fn(&mut App, &RequestContext) -> Result<Json, WgError>;

/// The feature's step for one slot of the maintenance plan. A slot no feature fills reports
/// `"not ported"` in the pass's report.
#[derive(Clone, Copy)]
pub struct MaintenanceStep {
    pub slot: Step,
    pub run: MaintenanceRun,
}

/// One feature's contributions. Every table may be empty.
pub struct Feature {
    /// The feature's name, as the crate names it (`decisions`, `tasks`, ...).
    pub name: &'static str,
    pub cli: &'static [CliGroup],
    pub routes: &'static [RouteGroup],
    pub tools: &'static [ToolRun],
    pub resources: &'static [ResourceRun],
    pub jobs: &'static [JobKind],
    pub maintenance: &'static [MaintenanceStep],
}
