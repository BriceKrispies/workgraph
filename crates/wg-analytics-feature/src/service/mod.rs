//! The analytics entry points of the application: `analytics metrics, query, overview, compare,
//! coupling, failures, history`.
//!
//! Each one checks its request as the Python model does, checks its permission, reads the source
//! rows of the warehouse in one transaction, builds the fact tables and hands the query to the
//! engine of `wg-analytics`. These are free functions over `&mut App`; the `cli` table reaches
//! them. The generation instants are the service clock's: the stamp of the permission check.

pub mod estimate;
pub mod export;
pub mod reads;
pub mod reports;
pub mod retrospective;
pub mod subagents;

use serde_json::Value as Json;
use wg_analytics::core::{coupling, failures, period::Period, service};
use wg_analytics::source::AnalyticsInput;
use wg_analytics::{Facts, build};
use wg_automation_feature::service::reads::finding_by_fingerprint;
use wg_json::Value;
use wg_model::{Row, Write};
use wg_registry::resolve;
use wg_runtime::{App, RequestContext, system_context};
use wg_types::{Id, Permission, Timestamp, WgError};
use wg_wire::convention::json_value;

use crate::spec::analyses::{self, Typed, compare_out};
use crate::spec::query::{self, QueryRequest};
use wg_types::pyval::text;

/// `AnalyticsService.require(ANALYTICS_READ)`; the stamp's clock reading is the service's `utcnow()`.
fn require(app: &mut App, context: &RequestContext) -> Result<Timestamp, WgError> {
    let stamp = app.runtime.stamps.stamp(context)?;
    stamp.require(&[Permission::AnalyticsRead])?;
    Ok(stamp.at)
}

/// What the commands read in one transaction: the workspace, the resolved project and repository
/// ids, and the source rows.
struct Loaded {
    workspace: Id,
    project: Option<String>,
    repository: Option<String>,
    input: AnalyticsInput,
}

/// Resolves the workspace and the optional project and repository (archived ones included) and
/// reads the warehouse copies.
fn load(
    app: &mut App,
    workspace: Option<&str>,
    project: Option<&str>,
    repository: Option<&str>,
) -> Result<Loaded, WgError> {
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.store.read(|session| {
        let slice = session.registry_world(Some(ws))?;
        let project = project
            .map(|reference| {
                resolve::project(&slice, ws, reference).map(|found| found.id.to_string())
            })
            .transpose()?;
        let repository = repository
            .map(|reference| {
                resolve::repository(&slice, ws, reference, true).map(|found| found.id.to_string())
            })
            .transpose()?;
        let input = reads::analytics_input(session)?;
        Ok(Loaded {
            workspace: ws,
            project,
            repository,
            input,
        })
    })
}

/// `AnalyticsService.query`: the request is checked first (the pydantic model runs before the
/// service), then the project and repository names are resolved in the workspace, the fact tables
/// are built from the warehouse copies, and the engine runs the query.
pub fn query(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &QueryRequest,
) -> Result<Value, WgError> {
    let metric = query::check_request(request)?;
    let at = require(app, context)?;
    let loaded = load(
        app,
        workspace,
        request.project.as_deref(),
        request.repository.as_deref(),
    )?;
    let facts = build(&loaded.input);
    let engine = query::engine_query(request, loaded.project, loaded.repository)?;
    let answer = query::run_query(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        &engine,
    )?;
    Ok(query::query_out(request, &metric, &answer, at))
}

/// The source rows with their fact tables, for the commands that read the whole warehouse.
fn facts_of(app: &mut App, workspace: Option<&str>) -> Result<(Loaded, Facts), WgError> {
    let loaded = load(app, workspace, None, None)?;
    let facts = build(&loaded.input);
    Ok((loaded, facts))
}

/// `AnalyticsService.overview`: the dashboard bundle for `period` buckets.
pub fn overview(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    period: &str,
) -> Result<Value, WgError> {
    query::check_inputs("tasks_created", period)?;
    require(app, context)?;
    let (loaded, facts) = facts_of(app, workspace)?;
    let bucket = Period::parse(period).unwrap_or(Period::Week);
    let bundle = service::overview(&loaded.input, &facts, &loaded.workspace.to_string(), bucket)
        .map_err(|err| query::refusal(&err))?;
    Ok(json_value(&bundle))
}

/// `AnalyticsService.compare`: one metric over two windows.
pub fn compare(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    metric: &str,
    first: Typed,
    second: Typed,
    group_by: &[String],
) -> Result<Value, WgError> {
    query::check_inputs(metric, "week")?;
    require(app, context)?;
    let (loaded, facts) = facts_of(app, workspace)?;
    let window = |(since, until): Typed| -> Result<(Timestamp, Timestamp), WgError> {
        Ok((query::instant(since)?, query::instant(until)?))
    };
    let grouped: Vec<&str> = group_by.iter().map(String::as_str).collect();
    let core = service::compare(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        metric,
        window(first)?,
        window(second)?,
        &grouped,
    )
    .map_err(|err| query::refusal(&err))?;
    Ok(compare_out(&core, first, second))
}

/// `AnalyticsService.coupling`: the repository co-change report.
pub fn coupling(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    project: Option<&str>,
    min_support: i64,
) -> Result<Value, WgError> {
    require(app, context)?;
    let (loaded, facts) = facts_of(app, workspace)?;
    let report = coupling::coupling_report(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        min_support,
        project,
    );
    Ok(json_value(&report))
}

/// `detect_failures`: the report of the workspace and the instant it was generated at.
fn failure_report(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<(Json, Id, Timestamp), WgError> {
    let at = require(app, context)?;
    let (loaded, facts) = facts_of(app, workspace)?;
    let report = failures::failures(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        &at.format_isoformat(),
        &analyses::fingerprint,
    );
    Ok((report, loaded.workspace, at))
}

/// `persist_findings`: records the report's findings, a new row per fingerprint and a refreshed
/// one for a finding seen before. Returns the number of findings seen.
fn persist_findings(app: &mut App, workspace: Option<&str>) -> Result<usize, WgError> {
    let context = system_context("failure-analysis")?;
    let (report, ws, at) = failure_report(app, &context, workspace)?;
    let stamp = app.runtime.stamps.stamp(&context)?;
    let findings: Vec<Json> = report
        .get("findings")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    app.runtime.store.write(|session| {
        findings
            .iter()
            .enumerate()
            .try_for_each(|(index, finding)| {
                let fingerprint = text(finding.get("fingerprint"));
                let stored = finding_by_fingerprint(session, ws, fingerprint)?;
                let id = stamp.id(u32::try_from(index).unwrap_or(u32::MAX));
                let write = match stored {
                    Some(row) => Write::update(Row::from(analyses::finding_row(
                        Some(row),
                        id,
                        ws,
                        finding,
                        at,
                    )?)),
                    None => {
                        Write::insert(Row::from(analyses::finding_row(None, id, ws, finding, at)?))
                    }
                };
                session.apply_write(&write)
            })
            .map(|()| findings.len())
    })
}

/// `AnalyticsService.failures`, after `persist_findings` when `persist` is set (as the command
/// does).
pub fn failures(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    persist: bool,
) -> Result<Value, WgError> {
    if persist {
        persist_findings(app, workspace)?;
    }
    let (report, _, _) = failure_report(app, context, workspace)?;
    Ok(json_value(&report))
}

/// `AnalyticsService.task_history`: the accepted tasks, newest first. An empty category or
/// repository is no filter (Python's truthiness).
pub fn history(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    category: Option<&str>,
    repository: Option<&str>,
    limit: i64,
) -> Result<Value, WgError> {
    require(app, context)?;
    let loaded = load(app, workspace, None, repository)?;
    let facts = build(&loaded.input);
    let rows = service::task_history(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        category,
        loaded.repository.as_deref(),
        usize::try_from(limit).unwrap_or(0),
    );
    Ok(json_value(&rows))
}
