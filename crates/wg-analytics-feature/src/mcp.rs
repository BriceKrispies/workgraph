//! The analytics tools and resources (`src/workgraph/mcp/server.py`, M59 to M63 and the
//! `workgraph://analytics/catalog` and `workgraph://workspaces/{workspace}/analytics/{metric}`
//! resources): `analytics_query`, `analytics_report`, `analytics_subagents`, `analytics_agent_time`
//! and `analytics_bottlenecks`.
//!
//! Each tool calls the [`crate::service`] entry point its CLI command or route calls, under the
//! agent's identity, so the permission checks are the services' own. The results are the same
//! documents the Python tools return, as structured output.

use serde_json::{Value, json};
use wg_iface::mcp::options::Agent;
use wg_iface::mcp::{structured_output, text_argument};
use wg_runtime::{App, RequestContext, ResourceRun, ToolRun, agent_context};
use wg_types::WgError;

use crate::service;
use crate::spec::params::{argument_date, json_strings};
use crate::spec::query::{self, QueryRequest};

/// The feature's tool descriptor: one [`call`] that answers the analytics tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// The feature's resource descriptor: the metric catalog and the metric by week.
pub const RESOURCES: &[ResourceRun] = &[resource];

/// Answers tool `name` when it is one of the analytics tools; `None` leaves it to the other
/// features.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "analytics_query" => query_tool(app, agent, arguments),
        "analytics_report" => report(app, agent, arguments),
        "analytics_subagents" => subagent_report(app, agent, arguments),
        "analytics_agent_time" => agent_time(app, agent, arguments),
        "analytics_bottlenecks" => bottlenecks(app, agent, arguments),
        _ => return None,
    };
    Some(answer)
}

/// The agent's context for a call.
fn context_of(agent: &Agent) -> Result<RequestContext, WgError> {
    agent_context(&agent.name, Some(agent.permissions.clone()))
}

/// `analytics_query` (M59): one predefined metric with its grouping and filters.
fn query_tool(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = context_of(agent)?;
    let request = QueryRequest {
        metric: text_argument(a, "metric").unwrap_or_default().to_owned(),
        group_by: json_strings(a.get("group_by")),
        period: text_argument(a, "period").unwrap_or("week").to_owned(),
        project: text_argument(a, "project").map(str::to_owned),
        repository: text_argument(a, "repository").map(str::to_owned),
        category: text_argument(a, "category").map(str::to_owned),
        provider: text_argument(a, "provider").map(str::to_owned),
        model: text_argument(a, "model").map(str::to_owned),
        since: None,
        until: None,
        limit: None,
    };
    let dto = service::query(app, &context, text_argument(a, "workspace"), &request)?;
    structured_output(&dto)
}

/// `analytics_report` (M60): the catalog, the overview, the coupling, the failures, the estimate
/// of a task, or the task history (`{"tasks": [...]}`).
fn report(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = context_of(agent)?;
    let workspace = text_argument(a, "workspace");
    match text_argument(a, "report").unwrap_or_default() {
        "catalog" => Ok(json!({ "metrics": structured_output(&query::definitions())? })),
        "overview" => structured_output(&service::overview(app, &context, workspace, "week")?),
        "coupling" => structured_output(&service::coupling(app, &context, workspace, None, 1)?),
        "failures" => structured_output(&service::failures(app, &context, workspace, false)?),
        "estimate" => {
            let task = text_argument(a, "task")
                .ok_or_else(|| WgError::validation("estimate requires a task"))?;
            structured_output(&service::estimate::estimate(
                app,
                &context,
                workspace,
                Some(task),
                None,
                "",
                None,
                &[],
            )?)
        }
        _ => {
            let rows = service::history(app, &context, workspace, None, None, 50)?;
            Ok(json!({ "tasks": structured_output(&rows)? }))
        }
    }
}

/// `analytics_subagents` (M61): the subagent runs by period and model.
fn subagent_report(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = context_of(agent)?;
    let period = text_argument(a, "period").unwrap_or("week");
    let dto = service::subagents::report(
        app,
        &context,
        text_argument(a, "workspace"),
        period,
        argument_date(a, "since")?,
        argument_date(a, "until")?,
        text_argument(a, "model"),
    )?;
    structured_output(&dto)
}

/// `analytics_agent_time` (M62): where subagent wall time went, by class or per group.
fn agent_time(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = context_of(agent)?;
    let dto = service::subagents::breakdown(
        app,
        &context,
        text_argument(a, "workspace"),
        argument_date(a, "since")?,
        argument_date(a, "until")?,
        text_argument(a, "by").unwrap_or("class"),
        text_argument(a, "model"),
    )?;
    structured_output(&dto)
}

/// `analytics_bottlenecks` (M63): time by class, the slowest calls and the wall time per task.
fn bottlenecks(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = context_of(agent)?;
    let top = a.get("top").and_then(Value::as_u64).unwrap_or(15);
    let dto = service::subagents::bottlenecks(
        app,
        &context,
        text_argument(a, "workspace"),
        argument_date(a, "since")?,
        argument_date(a, "until")?,
        usize::try_from(top).unwrap_or(usize::MAX),
    )?;
    structured_output(&dto)
}

/// Answers the analytics resources: `workgraph://analytics/catalog` and
/// `workgraph://workspaces/{workspace}/analytics/{metric}`. The text is Python's `json.dumps` of
/// the document.
fn resource(app: &mut App, agent: &Agent, uri: &str) -> Option<Result<String, WgError>> {
    if uri == "workgraph://analytics/catalog" {
        return Some(
            wg_json::log_json(&query::definitions())
                .map_err(|err| WgError::workgraph(err.to_string())),
        );
    }
    let rest = uri.strip_prefix("workgraph://workspaces/")?;
    let (workspace, tail) = rest.split_once('/')?;
    let metric = tail.strip_prefix("analytics/")?;
    Some(metric_by_week(app, agent, workspace, metric))
}

/// `res_metric`: the metric grouped by period, with the default week period.
fn metric_by_week(
    app: &mut App,
    agent: &Agent,
    workspace: &str,
    metric: &str,
) -> Result<String, WgError> {
    let context = context_of(agent)?;
    let request = QueryRequest {
        metric: metric.to_owned(),
        group_by: vec!["period".to_owned()],
        period: "week".to_owned(),
        ..QueryRequest::default()
    };
    let dto = service::query(app, &context, Some(workspace), &request)?;
    wg_json::log_json(&dto).map_err(|err| WgError::workgraph(err.to_string()))
}
