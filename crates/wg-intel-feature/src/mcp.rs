//! The classification tools (`src/workgraph/mcp/server.py`, M57 and M58): `classification_get`
//! and `classification_request`, and the search tool `task_search`.
//!
//! Each tool calls the [`crate::service::entry`] function its CLI command and route call, under the
//! agent's identity, so the permission checks are the services' own. A request queues a background
//! classification job with the default run options, as the Python tool does.

use serde_json::Value;
use wg_iface::mcp::options::Agent;
use wg_iface::mcp::{structured_output, text_argument};
use wg_json::Value as Dto;
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;

use crate::service::entry;
use crate::service::llm::Live;
use crate::spec::requests::{RunOptions, SearchRequest};
use crate::spec::search_out::SearchFilters;

/// The feature's tool descriptor: one [`call`] that answers the classification tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// Answers tool `name` when it is one of the classification tools; `None` leaves it to the other
/// features.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "classification_get" => get(app, agent, arguments),
        "classification_request" => request(app, agent, arguments),
        "task_search" => search(app, agent, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// `ClassificationService.get` for `classification_get` (M57): the task's classification and its
/// history.
fn get(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    entry::classify_show(
        app,
        &context,
        text_argument(a, "workspace"),
        text_argument(a, "task").unwrap_or_default(),
    )
}

/// `ClassificationService.request` for `classification_request` (M58): queues the job.
fn request(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    entry::classify_request(
        app,
        &context,
        text_argument(a, "workspace"),
        text_argument(a, "task").unwrap_or_default(),
        &RunOptions::default(),
    )
}

/// `SearchService.search` for `task_search`: the query in `hybrid` mode unless `mode` says otherwise,
/// twenty hits by default, and no filters.
fn search(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let request = SearchRequest {
        query: text_argument(a, "query").unwrap_or_default().to_owned(),
        mode: text_argument(a, "mode").unwrap_or("hybrid").to_owned(),
        limit: a.get("limit").and_then(Value::as_i64).unwrap_or(20),
        filters: SearchFilters::default(),
    };
    entry::search(
        app,
        &context,
        &mut Live,
        text_argument(a, "workspace"),
        &request,
    )
}
