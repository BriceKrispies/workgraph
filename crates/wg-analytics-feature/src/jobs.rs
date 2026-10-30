//! The `generate_report` job handler (`jobs/handlers.py::generate_report`): it runs
//! `generate_retrospective` for the job's workspace and scope and returns the report's id and its
//! narrative source. The worker collects [`JOBS`] through the feature's `FEATURE` value.

use crate::service::retrospective;
use crate::spec::args::scope_retrospective_options;
use serde_json::{Map, Value as Json, json};
use wg_runtime::{
    HandlerError, HandlerResult, JobContext, JobKind, Queue, RequestContext, system_context,
};
use wg_types::WgError;

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[JobKind {
    kind: "generate_report",
    handler: generate_report,
}];

/// `generate_report`: `generate_retrospective(workspace, **scope)`, with the context a system
/// component (`_ctx(job, "reports")`): the job's correlation and causation. The reply is the
/// report's `report_id` and `narrative_source`.
fn generate_report(queue: &mut Queue, job: &mut JobContext) -> HandlerResult {
    let context = match job.job.correlation_id.clone() {
        Some(correlation) => RequestContext::system("reports", correlation),
        None => system_context("reports"),
    }
    .map_err(HandlerError::Failed)?
    .caused_by(job.job.causation_id);
    let payload = &job.job.payload;
    let workspace = payload
        .get("workspace")
        .and_then(Json::as_str)
        .ok_or_else(|| HandlerError::Exception("KeyError: 'workspace'".to_owned()))?;
    let scope = payload.get("scope").and_then(Json::as_object);
    let options = scope_retrospective_options(scope);
    let report = retrospective::retrospective(&mut queue.app, &context, Some(workspace), &options)
        .map_err(HandlerError::Failed)?;
    let report = serde_json::to_value(&report)
        .map_err(|err| HandlerError::Failed(WgError::workgraph(err.to_string())))?;
    Ok(Some(Map::from_iter([
        ("report_id".to_owned(), json!(report["id"])),
        (
            "narrative_source".to_owned(),
            json!(report["narrative_source"]),
        ),
    ])))
}
