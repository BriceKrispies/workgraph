//! The analytics entry points of the application: `analytics metrics` and `analytics query`.
//!
//! Each one checks its request as the Python model does, checks its permission, reads the source
//! rows of the warehouse in one transaction, builds the fact tables and hands the query to the
//! engine of `wg-analytics`. These are free functions over `&mut App`; the `cli` table reaches
//! them.

pub mod reads;

use time::OffsetDateTime;
use wg_analytics::build;
use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, Timestamp, WgError};

use crate::spec::query::{self, QueryRequest};

/// `AnalyticsService.require(ANALYTICS_READ)`.
fn require(app: &mut App, context: &RequestContext) -> Result<(), WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::AnalyticsRead])
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
    require(app, context)?;
    let ws = app.workspace_or_default(workspace)?;
    let (project, repository, input) = app.runtime.store.read(|session| {
        let slice = session.registry_world(Some(ws))?;
        let project = request
            .project
            .as_deref()
            .map(|reference| {
                resolve::project(&slice, ws, reference).map(|found| found.id.to_string())
            })
            .transpose()?;
        let repository = request
            .repository
            .as_deref()
            .map(|reference| {
                resolve::repository(&slice, ws, reference, true).map(|found| found.id.to_string())
            })
            .transpose()?;
        let input = reads::analytics_input(session)?;
        Ok((project, repository, input))
    })?;
    let facts = build(&input);
    let engine = query::engine_query(request, project, repository)?;
    let answer = query::run_query(&input, &facts, &ws.to_string(), &engine)?;
    let generated_at = Timestamp::from_offset_datetime(OffsetDateTime::now_utc())?;
    Ok(query::query_out(request, &metric, &answer, generated_at))
}
