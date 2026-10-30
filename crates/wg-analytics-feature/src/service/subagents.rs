//! The subagent reports: `analytics subagents` (`SubagentReport.report`, M61, C014),
//! `analytics time` (`AgentTimeReport.breakdown`, M62, C015) and `analytics bottlenecks`
//! (`AgentTimeReport.bottlenecks`, M63, C003).
//!
//! The pure reports are the `wg-analytics` functions over the warehouse copy, with the
//! `subagent_runs` rows the reads copy. Each entry point checks its permission first, as the
//! Python service does, then its arguments, then reads the window and the scope.

use wg_analytics::agent_time;
use wg_analytics::common::Scope;
use wg_analytics::subagents::{self, Period};
use wg_json::LocalDateTime;
use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, WgError};
use wg_wire::convention::json_value;

use super::load;
use crate::spec::params::{report_window, time_group};

/// `require(ctx, Permission.TASK_READ)`: the reports read the task and run rows.
fn require(app: &mut App, context: &RequestContext) -> Result<(), WgError> {
    app.require_permission(context, Permission::TaskRead)
}

/// The scope of a report: the resolved workspace and the default one, when there is one.
fn scope(app: &mut App, workspace_id: String) -> Scope {
    Scope {
        workspace_id,
        default_workspace_id: app.default_workspace_id().ok().map(|id| id.to_string()),
    }
}

/// `SubagentReport.report`: the runs by period and model, with their linked task outcomes.
pub fn report(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    period: &str,
    since: Option<LocalDateTime>,
    until: Option<LocalDateTime>,
    model: Option<&str>,
) -> Result<Value, WgError> {
    require(app, context)?;
    let parsed = Period::parse(period).ok_or_else(|| {
        WgError::validation(format!("unknown period '{period}'"))
            .detail("allowed", serde_json::json!(["week", "month"]))
    })?;
    let window = report_window(since, until, model)?;
    let loaded = load(app, workspace, None, None)?;
    let scope = scope(app, loaded.workspace.to_string());
    Ok(json_value(&subagents::report(
        &loaded.input,
        &scope,
        parsed,
        &window,
    )))
}

/// `AgentTimeReport.breakdown`: the share of subagent wall time by tool class, or per group.
pub fn breakdown(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    since: Option<LocalDateTime>,
    until: Option<LocalDateTime>,
    by: &str,
    model: Option<&str>,
) -> Result<Value, WgError> {
    let by = time_group(by)?;
    require(app, context)?;
    let window = report_window(since, until, model)?;
    let loaded = load(app, workspace, None, None)?;
    let scope = scope(app, loaded.workspace.to_string());
    Ok(json_value(&agent_time::breakdown(
        &loaded.input,
        &scope,
        &window,
        by,
    )))
}

/// `AgentTimeReport.bottlenecks`: time by class, the slowest calls with the runs active at their
/// start, and wall time per task. `top` bounds the slowest calls and the task rows.
pub fn bottlenecks(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    since: Option<LocalDateTime>,
    until: Option<LocalDateTime>,
    top: usize,
) -> Result<Value, WgError> {
    require(app, context)?;
    let window = report_window(since, until, None)?;
    let loaded = load(app, workspace, None, None)?;
    let scope = scope(app, loaded.workspace.to_string());
    Ok(json_value(&agent_time::bottlenecks(
        &loaded.input,
        &scope,
        &window,
        top,
    )))
}
