//! Shell: the classification writes (`ClassificationService`): request, run, promote, confirm,
//! reclassify, gold labels and evaluations. Each one loads its rows, takes the decision from
//! `wg-intel` and applies the writes and events in one transaction (the runtime's decide cycle).
//!
//! The provider calls happen between the transactions, as Python's services do; a failed call is
//! stored as a failed classification row before the error is raised or returned.

use serde_json::Value as Json;
use wg_intel::{
    CATEGORIES, PROMPT_VERSION, RunStatus, SYSTEM_PROMPT, TaskText, apply_current,
    confirm as confirm_plan, evaluation_result, evaluation_row, gold_label, model_row,
    output_schema, promote as promote_plan, schema_error_text, validate_classification_errors,
    validation_error_text,
};
use wg_jobs::queue::NewJob;
use wg_model::entity::{
    Classification, ClassificationEvaluation, ClassificationGoldLabel, JsonObject, Task,
};
use wg_model::event::classification::{
    ClassificationCompleted, ClassificationConfirmed, ClassificationDisagreement,
    ClassificationEvaluated, ClassificationFailed, ClassificationPromoted,
    ClassificationSuperseded,
};
use wg_model::{Decision, EventDraft, EventPayload, Row, RowKey, Write};
use wg_netspec::ProviderError;
use wg_registry::resolve;
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_store::Session;
use wg_types::pyval::{prefix, text};
use wg_types::{Correlation, Id, Permission, WgError};

use crate::service::classify as classify_reads;
use crate::service::llm::Providers;
use crate::spec::llm::selection;
use crate::spec::llm::{CallError, Generation, is_unavailable, unavailable_error};
use crate::spec::plan::{
    Attempt, event, ids_of, job_payload, prompt_of, text_or_none, text_or_null, unknown_category,
    validation_status,
};
use crate::spec::requests::{ConfirmRequest, ReclassifyRequest, RunOptions};

/// The job kind of one classification run.
pub const CLASSIFY_JOB: &str = "classify_task";

/// The GPU resource key classification and embedding jobs take.
pub const GPU_RESOURCE: &str = "gpu";

pub struct Reclassified {
    pub batch_id: Id,
    pub job_ids: Vec<Id>,
    pub total_matching: usize,
}

/// The result of an evaluation: the stored row, its metrics and the per-label results.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    pub row: ClassificationEvaluation,
    pub metrics: Json,
    pub results: Vec<JsonObject>,
    pub provider: String,
    pub model: String,
}

/// A task as the classification writes read it: the row and its tags.
type Loaded = (Task, Vec<String>);

/// How much of a failed evaluation sample's error is stored (`str(exc)[:300]` in Python).
const EVALUATION_ERROR_CHARS: usize = 300;

/// The task with `id` and its tags (`session.get(Task, ...)` plus the tag rows).
pub fn load_task(session: &mut Session<'_>, id: Id) -> Result<Loaded, WgError> {
    let task = session
        .fetch::<Task>(&RowKey::Task(id))?
        .ok_or_else(|| WgError::not_found(format!("task {id} not found")))?;
    let world = session.task_world(Some(task.workspace_id), &[])?;
    let tags = world.tasks.tags_of(id).map(str::to_owned).collect();
    Ok((task, tags))
}

/// The current classification row of `task`, if it has one.
fn current_of(session: &mut Session<'_>, task: &Task) -> Result<Option<Classification>, WgError> {
    match task.current_classification_id {
        Some(id) => session.fetch::<Classification>(&RowKey::Classification(id)),
        None => Ok(None),
    }
}

/// `ClassificationService.request`: queues a `classify_task` job for the task. Returns the job id,
/// the task and its key, and the job's status.
pub fn request(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    options: &RunOptions,
) -> Result<(Id, Task, String), WgError> {
    request_with_correlation(app, context, workspace, task_ref, options, None)
}

/// [`request`] with the job's `correlation_id` set to `correlation` when one is given. Python's
/// automation actions pass the run's correlation (`ctx.correlation_id`) to the job they queue.
pub fn request_with_correlation(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    options: &RunOptions,
    correlation: Option<Correlation>,
) -> Result<(Id, Task, String), WgError> {
    app.require_permission(context, Permission::ClassifyRequest)?;
    let ws = workspace
        .map(|reference| app.workspace_id(reference))
        .transpose()?;
    let task = app.runtime.query(
        |session| {
            let world = session.task_world(ws, &[])?;
            Ok(resolve::task(&world.tasks, ws, task_ref)?.clone())
        },
        |task| Ok(task.clone()),
    )?;
    let suffix = app.runtime.stamps.stamp(context)?.id(0).to_string();
    let key = format!(
        "classify:{}:{}:{}:{}:{}:{}",
        task.id,
        task.description_hash,
        text_or_none(&options.provider),
        text_or_none(&options.model),
        options.promote,
        suffix.get(..8).unwrap_or(&suffix),
    );
    let job = NewJob {
        payload: job_payload(vec![
            ("task_id", Json::String(task.id.to_string())),
            ("provider", text_or_null(&options.provider)),
            ("model", text_or_null(&options.model)),
            ("promote", Json::Bool(options.promote)),
            ("reason", Json::String("requested".to_owned())),
        ]),
        workspace_id: Some(task.workspace_id),
        task_id: Some(task.id),
        resource_keys: vec![GPU_RESOURCE.to_owned()],
        max_attempts: 4,
        backoff_base_seconds: 15.0,
        correlation_id: correlation,
        idempotency_key: Some(key),
        created_by: context.actor.to_string(),
        ..NewJob::new(CLASSIFY_JOB)
    };
    let job_id = app.enqueue_as(context, &job)?;
    let status = app
        .runtime
        .store
        .read(|session| session.job(job_id))?
        .ok_or_else(|| WgError::not_found(format!("job {job_id} not found")))?
        .status
        .as_str()
        .to_owned();
    Ok((job_id, task, status))
}

/// `ClassificationService.run_classification`: classifies the task with the classification
/// provider (or the given one), stores the result as a row, and applies the current-classification
/// policy. A succeeded row for the same provider, model, prompt and input is returned as it is,
/// unless `promote` is set.
pub fn run_classification(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    task_id: Id,
    options: &RunOptions,
) -> Result<Classification, WgError> {
    let (task, tags) = app.runtime.query(
        |session| load_task(session, task_id),
        |loaded| Ok(loaded.clone()),
    )?;
    let slug = app.workspace_slug(&task.workspace_id.to_string())?;
    let config = app.effective_config(Some(&slug))?;
    let (default_provider, default_model) = selection(&config, "classification")?;
    let provider = options.provider.clone().unwrap_or(default_provider);
    let model = options.model.clone().unwrap_or(default_model);
    let text = TaskText::of(&task);
    let digest = text.digest();
    if !options.promote {
        let existing = app.runtime.query(
            |session| {
                classify_reads::succeeded_classification(
                    session,
                    task_id,
                    &provider,
                    &model,
                    PROMPT_VERSION,
                    &digest,
                )
            },
            |row| Ok(row.clone()),
        )?;
        if let Some(row) = existing {
            return Ok(row);
        }
    }
    let run = Attempt {
        task_id,
        provider: provider.clone(),
        model: model.clone(),
        digest: digest.clone(),
        batch_id: options.batch_id,
    };
    let mut llm = providers.provider(&provider, &config)?;
    let prompt = prompt_of(&text.title, &text.description, &text.criteria, &tags);
    let outcome = llm.generate_json(&model, SYSTEM_PROMPT, &prompt, &output_schema());
    match outcome {
        Ok(Generation { data, duration_ms }) => {
            let duration = i64::try_from(duration_ms).unwrap_or(i64::MAX);
            match validate_classification_errors(&Json::Object(data.clone())) {
                Ok(parsed) => record_success(
                    app,
                    context,
                    &run,
                    &parsed,
                    &data,
                    duration,
                    options.promote,
                ),
                Err(errors) => record_failure(
                    app,
                    context,
                    &run,
                    "invalid_output",
                    &format!(
                        "output failed schema validation: {}",
                        schema_error_text(&errors)
                    ),
                    Some(&data),
                    Some(duration),
                ),
            }
        }
        Err(CallError::Provider(error)) if is_unavailable(&error) => {
            record_failure(
                app,
                context,
                &run,
                "provider_unavailable",
                error.message(),
                None,
                None,
            )?;
            Err(unavailable_error(&error))
        }
        Err(CallError::Provider(ProviderError::InvalidOutput(message))) => {
            record_failure(app, context, &run, "invalid_output", &message, None, None)
        }
        Err(CallError::Provider(error)) => Err(unavailable_error(&error)),
        Err(CallError::Unsupported(message)) => {
            record_failure(app, context, &run, "error", &message, None, None)
        }
        Err(CallError::Raised(error)) => Err(error),
    }
}

/// Stores a failed run (`_store_failure`) and records `classification.failed`.
fn record_failure(
    app: &mut App,
    context: &RequestContext,
    run: &Attempt,
    validation: &str,
    error: &str,
    raw: Option<&JsonObject>,
    duration_ms: Option<i64>,
) -> Result<Classification, WgError> {
    let outcome = app.runtime.decide(
        context,
        |session| load_task(session, run.task_id),
        |(task, _), stamp| {
            let ids = ids_of(stamp, 2);
            let row = model_row(
                &run.run_row(ids[0], stamp.actor.to_string(), stamp.at),
                RunStatus::Failed {
                    validation: validation_status(validation),
                    error,
                    raw,
                    duration_ms,
                },
            );
            let payload = ClassificationFailed {
                classification_id: row.id,
                error: Some(prefix(error, 500)),
                model: Some(run.model.clone()),
                provider: Some(run.provider.clone()),
                reason: validation.to_owned(),
            };
            Ok(Decision::new(row.clone())
                .write(Write::insert(Row::Classification(row)))
                .event(event(
                    stamp,
                    ids[1],
                    task.workspace_id,
                    task.id,
                    EventPayload::ClassificationFailed(payload),
                )))
        },
    )?;
    Ok(outcome.reply)
}

/// Stores a validated run and applies the current-classification policy (the success branch of
/// `run_classification`), then re-indexes the task.
fn record_success(
    app: &mut App,
    context: &RequestContext,
    run: &Attempt,
    parsed: &Json,
    raw: &JsonObject,
    duration_ms: i64,
    promote: bool,
) -> Result<Classification, WgError> {
    let outcome = app.runtime.decide_then(
        context,
        |session| {
            let (task, tags) = load_task(session, run.task_id)?;
            let current = current_of(session, &task)?;
            Ok((task, tags, current))
        },
        |(task, _, current), stamp| {
            let ids = ids_of(stamp, 4);
            let row = model_row(
                &run.run_row(ids[0], stamp.actor.to_string(), stamp.at),
                RunStatus::Succeeded {
                    parsed,
                    raw,
                    duration_ms,
                },
            );
            let plan = apply_current(task, current.as_ref(), row, promote);
            let mut decision = Decision::new(plan.row.clone())
                .write(Write::insert(Row::Classification(plan.row.clone())));
            if let Some(previous) = plan.previous.clone() {
                decision = decision.write(Write::update(Row::Classification(previous)));
            }
            if let Some(updated) = plan.task.clone() {
                decision = decision.write(Write::update(Row::Task(updated)));
            }
            let mut payloads = Vec::new();
            if let Some(disagreement) = &plan.disagreement {
                payloads.push(EventPayload::ClassificationDisagreement(
                    ClassificationDisagreement {
                        current: disagreement.get("current").cloned().unwrap_or(Json::Null),
                        new: disagreement.get("new").cloned().unwrap_or(Json::Null),
                        replaced: disagreement.get("replaced").cloned().unwrap_or(Json::Null),
                    },
                ));
            }
            if let Some(superseded) = &plan.superseded {
                payloads.push(EventPayload::ClassificationSuperseded(
                    ClassificationSuperseded {
                        new_id: superseded.new_id,
                        previous_id: superseded.previous_id,
                        reason: superseded.reason.to_owned(),
                    },
                ));
            }
            payloads.push(EventPayload::ClassificationCompleted(
                ClassificationCompleted {
                    category: plan.row.category.clone(),
                    classification_id: plan.row.id,
                    confidence: plan.row.confidence,
                    is_current: plan.row.is_current,
                    model: Some(run.model.clone()),
                    prompt_version: Some(PROMPT_VERSION.to_owned()),
                    provider: Some(run.provider.clone()),
                    taxonomy_version: wg_intel::TAXONOMY_VERSION.to_owned(),
                },
            ));
            let events = payloads
                .into_iter()
                .zip(ids.into_iter().skip(1))
                .map(|(payload, id)| event(stamp, id, task.workspace_id, task.id, payload));
            let reply = plan.row.clone();
            Ok(events.fold(decision, Decision::event).with_reply(reply))
        },
        |session, _| session.index_task(run.task_id),
    )?;
    Ok(outcome.reply)
}

/// `ClassificationService.promote`: makes a succeeded row the current one.
pub fn promote(
    app: &mut App,
    context: &RequestContext,
    classification_id: Id,
) -> Result<Classification, WgError> {
    app.require_permission(context, Permission::ClassifyConfirm)?;
    let outcome = app.runtime.decide_then(
        context,
        |session| {
            let row = session
                .fetch::<Classification>(&RowKey::Classification(classification_id))?
                .filter(|row| row.status == "succeeded")
                .ok_or_else(|| {
                    WgError::not_found(format!(
                        "successful classification {classification_id} not found"
                    ))
                })?;
            let (task, _) = load_task(session, row.task_id)?;
            let current = current_of(session, &task)?;
            Ok((row, task, current))
        },
        |(row, task, current), stamp| {
            let ids = ids_of(stamp, 1);
            let plan = promote_plan(task, current.as_ref(), row);
            let mut decision = Decision::new(plan.row.clone())
                .write(Write::update(Row::Classification(plan.row.clone())));
            if let Some(previous) = plan.previous.clone() {
                decision = decision.write(Write::update(Row::Classification(previous)));
            }
            let decision = decision.write(Write::update(Row::Task(plan.task.clone())));
            let payload = ClassificationPromoted {
                classification_id: plan.row.id,
                previous_id: plan.previous_id,
            };
            let reply = plan.row.clone();
            Ok(decision
                .event(event(
                    stamp,
                    ids[0],
                    task.workspace_id,
                    task.id,
                    EventPayload::ClassificationPromoted(payload),
                ))
                .with_reply(reply))
        },
        |session, _| {
            let task_id = classification_id_task(session, classification_id)?;
            session.index_task(task_id)
        },
    )?;
    Ok(outcome.reply)
}

/// The task a classification row belongs to, for the index step of a promotion.
fn classification_id_task(session: &mut Session<'_>, classification_id: Id) -> Result<Id, WgError> {
    session
        .fetch::<Classification>(&RowKey::Classification(classification_id))?
        .map(|row| row.task_id)
        .ok_or_else(|| {
            WgError::not_found(format!(
                "successful classification {classification_id} not found"
            ))
        })
}

/// `ClassificationService.confirm`: records a human classification. It becomes current and is
/// never replaced by a model. With `add_to_gold` it also joins the gold set.
pub fn confirm(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    request: &ConfirmRequest,
) -> Result<Classification, WgError> {
    app.require_permission(context, Permission::ClassifyConfirm)?;
    if !CATEGORIES.contains(&request.category.as_str()) {
        return Err(
            unknown_category(&request.category).detail("categories", serde_json::json!(CATEGORIES))
        );
    }
    let ws = workspace
        .map(|reference| app.workspace_id(reference))
        .transpose()?;
    let outcome = app.runtime.decide_then(
        context,
        |session| {
            let world = session.task_world(ws, &[])?;
            let task = resolve::task(&world.tasks, ws, task_ref)?.clone();
            let current = current_of(session, &task)?;
            Ok((task, current))
        },
        |(task, current), stamp| {
            let ids = ids_of(stamp, 3);
            let plan = confirm_plan(&wg_intel::Confirmation {
                id: ids[0],
                task,
                current: current.as_ref(),
                category: &request.category,
                subtype: request.subtype.as_deref(),
                domain: request.domain.as_deref(),
                complexity: request.complexity.as_deref(),
                tags: &request.tags,
                note: &request.note,
                actor: &stamp.actor.to_string(),
                created_at: stamp.at,
            });
            let mut decision = Decision::new(plan.row.clone())
                .write(Write::insert(Row::Classification(plan.row.clone())));
            if let Some(previous) = plan.previous.clone() {
                decision = decision.write(Write::update(Row::Classification(previous)));
            }
            let decision = decision.write(Write::update(Row::Task(plan.task.clone())));
            let decision = if request.add_to_gold {
                let label = gold_label(
                    ids[1],
                    task.workspace_id,
                    Some(task),
                    ("", ""),
                    &request.category,
                    &stamp.actor.to_string(),
                    stamp.at,
                );
                decision.write(Write::insert(Row::ClassificationGoldLabel(label)))
            } else {
                decision
            };
            let payload = ClassificationConfirmed {
                category: Some(request.category.clone()),
                classification_id: plan.row.id,
                previous: current.as_ref().map(|current| {
                    Json::Object(
                        [
                            ("category".to_owned(), text_or_null(&current.category)),
                            ("model".to_owned(), text_or_null(&current.model)),
                        ]
                        .into_iter()
                        .collect(),
                    )
                }),
            };
            let reply = plan.row.clone();
            Ok(decision
                .event(event(
                    stamp,
                    ids[2],
                    task.workspace_id,
                    task.id,
                    EventPayload::ClassificationConfirmed(payload),
                ))
                .with_reply(reply))
        },
        |session, _| {
            let task_id = current_task_id(session, ws, task_ref)?;
            session.index_task(task_id)
        },
    )?;
    Ok(outcome.reply)
}

/// The id of the task a reference names, for the index step of a confirmation.
fn current_task_id(
    session: &mut Session<'_>,
    ws: Option<Id>,
    task_ref: &str,
) -> Result<Id, WgError> {
    let world = session.task_world(ws, &[])?;
    Ok(resolve::task(&world.tasks, ws, task_ref)?.id)
}

/// `ClassificationService.reclassify`: queues a `classify_task` job for every task of the
/// workspace (not archived, optionally of one category, at most 500), as one batch.
pub fn reclassify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &ReclassifyRequest,
) -> Result<Reclassified, WgError> {
    app.require_permission(context, Permission::ClassifyRequest)?;
    let ws = app.workspace_or_default(workspace)?;
    let batch_id = app.runtime.stamps.stamp(context)?.id(0);
    let (tasks, total) = app.runtime.query(
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            let mut matching: Vec<Task> = world
                .tasks
                .tasks
                .values()
                .filter(|task| task.archived_at.is_none())
                .filter(|task| {
                    request
                        .category
                        .as_deref()
                        .is_none_or(|category| task.category.as_deref() == Some(category))
                })
                .cloned()
                .collect();
            matching.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then(b.number.cmp(&a.number))
            });
            let total = matching.len();
            matching.truncate(500);
            Ok((matching, total))
        },
        |(tasks, total)| Ok((tasks.clone(), *total)),
    )?;
    let mut job_ids = Vec::new();
    for task in &tasks {
        let job = NewJob {
            payload: job_payload(vec![
                ("task_id", Json::String(task.id.to_string())),
                ("provider", text_or_null(&request.provider)),
                ("model", text_or_null(&request.model)),
                ("promote", Json::Bool(request.promote)),
                ("batch_id", Json::String(batch_id.to_string())),
            ]),
            workspace_id: Some(task.workspace_id),
            task_id: Some(task.id),
            resource_keys: vec![GPU_RESOURCE.to_owned()],
            max_attempts: 4,
            backoff_base_seconds: 15.0,
            idempotency_key: Some(format!("reclassify:{batch_id}:{}", task.id)),
            created_by: context.actor.to_string(),
            ..NewJob::new(CLASSIFY_JOB)
        };
        job_ids.push(app.enqueue_as(context, &job)?);
    }
    Ok(Reclassified {
        batch_id,
        job_ids,
        total_matching: total,
    })
}

/// `ClassificationService.add_gold`: a gold label from a task or from a title and description.
/// Returns the label row.
pub fn add_gold(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    category: &str,
    task_ref: Option<&str>,
    title: Option<&str>,
    description: &str,
) -> Result<ClassificationGoldLabel, WgError> {
    app.require_permission(context, Permission::ClassifyConfirm)?;
    if !CATEGORIES.contains(&category) {
        return Err(WgError::validation(format!(
            "unknown category '{category}'"
        )));
    }
    let ws = app.workspace_or_default(workspace)?;
    let outcome = app.runtime.decide(
        context,
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            task_ref
                .map(|reference| Ok(resolve::task(&world.tasks, Some(ws), reference)?.clone()))
                .transpose()
        },
        |task, stamp| {
            if task.is_none() && title.is_none_or(str::is_empty) {
                return Err(WgError::validation(
                    "provide a task or a title for the gold label",
                ));
            }
            let id = ids_of(stamp, 1)[0];
            let label = gold_label(
                id,
                ws,
                task.as_ref(),
                (title.unwrap_or_default(), description),
                category,
                &stamp.actor.to_string(),
                stamp.at,
            );
            Ok(wg_model::Decision::new(label.clone())
                .write(Write::insert(Row::ClassificationGoldLabel(label))))
        },
    )?;
    Ok(outcome.reply)
}

/// `ClassificationService.evaluate`: runs a provider and model over the gold set, stores the
/// evaluation with its metrics, and records `classification.evaluated`.
pub fn evaluate(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    workspace: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Evaluated, WgError> {
    app.require_permission(context, Permission::AnalyticsRead)?;
    let ws = app.workspace_or_default(workspace)?;
    let slug = app.workspace_slug(&ws.to_string())?;
    let labels = app.runtime.query(
        |session| {
            Ok(classify_reads::gold_labels(session, ws)?
                .into_iter()
                .map(|label| (label.id, label.title, label.description, label.category))
                .collect::<Vec<_>>())
        },
        |labels| Ok(labels.clone()),
    )?;
    if labels.is_empty() {
        return Err(WgError::validation(
            "no gold-standard labels exist; confirm classifications or add gold labels first",
        ));
    }
    let config = app.effective_config(Some(&slug))?;
    let (default_provider, default_model) = selection(&config, "classification")?;
    let provider_name = provider.map_or(default_provider, str::to_owned);
    let model_name = model.map_or(default_model, str::to_owned);
    let mut llm = providers.provider(&provider_name, &config)?;
    let mut results = Vec::new();
    for (label_id, title, description, expected) in &labels {
        let prompt = prompt_of(title, description, &[], &[]);
        let outcome = llm.generate_json(&model_name, SYSTEM_PROMPT, &prompt, &output_schema());
        let entry = match outcome {
            Ok(Generation { data, .. }) => {
                match validate_classification_errors(&Json::Object(data)) {
                    Ok(parsed) => {
                        let predicted = text(parsed.get("category"));
                        evaluation_result(*label_id, title, expected, Ok(predicted))
                    }
                    Err(errors) => {
                        let text = validation_error_text("TaskClassification", &errors);
                        evaluation_result(
                            *label_id,
                            title,
                            expected,
                            Err(&prefix(&text, EVALUATION_ERROR_CHARS)),
                        )
                    }
                }
            }
            Err(CallError::Provider(error)) if is_unavailable(&error) => {
                return Err(unavailable_error(&error));
            }
            Err(CallError::Provider(error)) => evaluation_result(
                *label_id,
                title,
                expected,
                Err(&prefix(error.message(), EVALUATION_ERROR_CHARS)),
            ),
            Err(CallError::Unsupported(message)) => evaluation_result(
                *label_id,
                title,
                expected,
                Err(&prefix(&message, EVALUATION_ERROR_CHARS)),
            ),
            Err(CallError::Raised(error)) => return Err(error),
        };
        results.push(entry);
    }
    let outcome = app.runtime.decide(
        context,
        |_| Ok(()),
        |_, stamp| {
            let ids = ids_of(stamp, 2);
            let (row, metrics) =
                evaluation_row(ids[0], ws, &provider_name, &model_name, &results, stamp.at);
            let payload = ClassificationEvaluated {
                accuracy: row.accuracy,
                evaluation_id: row.id,
                model: model_name.clone(),
                provider: provider_name.clone(),
                sample_size: row.sample_size,
            };
            let reply = Evaluated {
                row: row.clone(),
                metrics,
                results: results.clone(),
                provider: provider_name.clone(),
                model: model_name.clone(),
            };
            Ok(Decision::new(reply)
                .write(Write::insert(Row::ClassificationEvaluation(row)))
                .event(EventDraft::new(
                    stamp,
                    ids[1],
                    Some(ws),
                    ws,
                    EventPayload::ClassificationEvaluated(payload),
                )))
        },
    )?;
    Ok(outcome.reply)
}
