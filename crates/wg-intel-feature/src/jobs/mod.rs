//! Shell: the job handlers of the intelligence feature (`jobs/handlers.py`): `classify_task`,
//! `embed_task` and `evaluate_classifier`. Each runs outside any transaction and calls the service
//! entry point of its work with the job's correlation and causation.

use serde_json::{Map, Value as Json};
use wg_runtime::{HandlerError, HandlerResult, JobContext, JobKind, Queue, system_context};
use wg_runtimespec::RequestContext;
use wg_types::{ErrorCode, WgError};

use crate::service::embed;
use crate::service::llm::Live;
use crate::service::ops;
use crate::spec::payload::{flag, id_of, text};
use crate::spec::requests::RunOptions;
use crate::spec::write_out;

/// The job kinds of the feature with their handlers.
pub static JOBS: &[JobKind] = &[
    JobKind {
        kind: "classify_task",
        handler: classify_task,
    },
    JobKind {
        kind: "embed_task",
        handler: embed_task,
    },
    JobKind {
        kind: "evaluate_classifier",
        handler: evaluate_classifier,
    },
];

/// `classify_task`: classifies one task with the job's provider and model.
fn classify_task(queue: &mut Queue, job: &mut JobContext) -> HandlerResult {
    let context = match job.job.correlation_id.clone() {
        Some(correlation) => RequestContext::system("classifier", correlation),
        None => system_context("classifier"),
    }
    .map_err(HandlerError::Failed)?
    .caused_by(job.job.causation_id);
    let payload = job.job.payload.clone();
    let options = RunOptions {
        provider: text(&payload, "provider").map(str::to_owned),
        model: text(&payload, "model").map(str::to_owned),
        promote: flag(&payload, "promote"),
        batch_id: match text(&payload, "batch_id") {
            Some(_) => Some(id_of(&payload, "batch_id").map_err(HandlerError::Failed)?),
            None => None,
        },
    };
    let task_id = id_of(&payload, "task_id").map_err(HandlerError::Failed)?;
    let row = ops::run_classification(&mut queue.app, &context, &mut Live, task_id, &options)
        .map_err(HandlerError::Failed)?;
    Ok(Some(write_out::classify_result(&row)))
}

/// `embed_task`: embeds one task under the job's provider and model.
fn embed_task(queue: &mut Queue, job: &mut JobContext) -> HandlerResult {
    let context = match job.job.correlation_id.clone() {
        Some(correlation) => RequestContext::system("embedder", correlation),
        None => system_context("embedder"),
    }
    .map_err(HandlerError::Failed)?
    .caused_by(job.job.causation_id);
    let payload = job.job.payload.clone();
    let task_id = id_of(&payload, "task_id").map_err(HandlerError::Failed)?;
    let embedded = embed::embed_task(
        &mut queue.app,
        &context,
        &mut Live,
        task_id,
        text(&payload, "provider"),
        text(&payload, "model"),
    )
    .map_err(HandlerError::Failed)?;
    Ok(Some(write_out::embed_result(
        embedded.task_id,
        embedded.status,
        &embedded.provider,
        &embedded.model,
        embedded.dimensions,
    )))
}

/// `evaluate_classifier`: evaluates the job's provider and model over the gold set of a workspace.
fn evaluate_classifier(queue: &mut Queue, job: &mut JobContext) -> HandlerResult {
    let context = match job.job.correlation_id.clone() {
        Some(correlation) => RequestContext::system("classifier", correlation),
        None => system_context("classifier"),
    }
    .map_err(HandlerError::Failed)?
    .caused_by(job.job.causation_id);
    let payload: Map<String, Json> = job.job.payload.clone();
    let workspace = text(&payload, "workspace").ok_or_else(|| {
        HandlerError::Failed(WgError::new(
            ErrorCode::ValidationFailed,
            "job payload has no workspace",
        ))
    })?;
    let evaluated = ops::evaluate(
        &mut queue.app,
        &context,
        &mut Live,
        Some(workspace),
        text(&payload, "provider"),
        text(&payload, "model"),
    )
    .map_err(HandlerError::Failed)?;
    Ok(Some(write_out::evaluate_result(&evaluated.row)))
}
