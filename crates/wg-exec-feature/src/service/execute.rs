//! Shell: `RunService.execute`, the `agent_run` job: prepares the attempt's worktrees, records the
//! prompt, launches the agent outside any database transaction while its events stream into attempt
//! events, captures its log and result, and concludes the attempt.
//!
//! The agent is launched through an [`AgentHost`], so tests run a fake adapter. The agent's events
//! reach the database from the cancellation probe the host polls (the process library calls it on
//! this thread), and once more when the agent has finished.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::{Map, Value as Json, json};
use wg_agentspec::{AgentError, AgentEvent, AgentRunRequest, EventKind};
use wg_exec::run::{
    Checkout, EarlyEnd, ExecuteEntry, adapter_crash_result, agent_env, cancel_poll_due,
    choose_workdir, execute_entry, flush_due, provider_unavailable_result, records_event,
    timeout_seconds, usage_fields,
};
use wg_runtime::{App, JobContext, system_context};
use wg_runtimespec::RequestContext;
use wg_types::{ErrorCode, Id, WgError, parse_id};

use crate::service::agents::{AgentHost, require_available};
use crate::service::conclude::{Collaborators, Concluding, conclude_attempt};
use crate::service::worktrees::{worktree_prepare, worktree_release};
use crate::spec::artifacts::{ArtifactSpec, artifact_row, stored_path};
use crate::spec::conclude::{AgentReport, EndFacts, RunRecord, finalize_failure};
use crate::spec::config::execution_number;
use crate::spec::execute::{
    bind_session_decision, environment, events_decision, missing_payload, preparing_decision,
    running_decision, synthetic_result,
};
use crate::spec::prompt::{PROMPT_VERSION, PromptRepo, build_prompt};
use crate::spec::sql::session_by_external;
use crate::spec::worktrees::PrepareRequest;
use wg_types::pyval::text;

/// The name of the file the agent's output is logged to.
const LOG_NAME: &str = "agent.log";

/// What the recorder knows between two polls.
struct Recorder {
    context: RequestContext,
    workspace: Id,
    attempt: Id,
    adapter: String,
    provider: String,
    /// Progress events recorded so far.
    count: usize,
    last_write: Instant,
    last_poll: Instant,
    cancelled: bool,
}

type Pending = Arc<Mutex<Vec<AgentEvent>>>;

/// `_EventRecorder`: writes what the agent streamed since the last poll. The session an agent
/// reports is bound at once; the events are written when one of them is a progress, result or
/// session event, or a second has passed since the last write (`force` writes regardless).
fn drain(app: &mut App, state: &mut Recorder, pending: &Pending, force: bool) {
    let mut batch = std::mem::take(&mut *pending.lock().unwrap_or_else(PoisonError::into_inner));
    if batch.is_empty() {
        return;
    }
    for event in &batch {
        if event.kind == EventKind::Session
            && let Some(session) = event
                .data
                .get("session_id")
                .and_then(Json::as_str)
                .filter(|id| !id.is_empty())
        {
            let model = event.data.get("model").and_then(Json::as_str);
            // Recording is best effort: a failed bind never stops the agent.
            let _ = bind_session(app, state, session, model);
        }
    }
    let mut seen = state.count;
    batch.retain(|event| {
        let keep = records_event(event.kind.as_str(), seen);
        if keep {
            seen += 1;
        }
        keep
    });
    let since = state.last_write.elapsed().as_secs_f64();
    let due = force
        || batch
            .iter()
            .any(|event| flush_due(event.kind.as_str(), since));
    if !due {
        let mut queue = pending.lock().unwrap_or_else(PoisonError::into_inner);
        batch.append(&mut queue);
        *queue = batch;
        return;
    }
    let recorded = state.count;
    let (workspace, attempt) = (state.workspace, state.attempt);
    let written = app
        .runtime
        .decide(
            &state.context,
            |_| Ok(()),
            |(), stamp| {
                Ok(events_decision(
                    stamp,
                    stamp.ids(),
                    workspace,
                    attempt,
                    &batch,
                    recorded,
                ))
            },
        )
        .map(|outcome| outcome.reply)
        .unwrap_or(0);
    state.count += written;
    state.last_write = Instant::now();
}

fn bind_session(
    app: &mut App,
    state: &Recorder,
    session_id: &str,
    model: Option<&str>,
) -> Result<(), WgError> {
    let key = wg_exec::run::session_provider_key(&state.adapter, &state.provider).to_owned();
    let attempt_id = state.attempt;
    app.runtime
        .decide(
            &state.context,
            |session| {
                let attempt = session
                    .attempt_by_id(attempt_id)?
                    .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
                let existing = session
                    .rows(&session_by_external(&key, session_id))?
                    .first()
                    .map(|values| {
                        <wg_model::entity::AgentSession as wg_storecodec::TableRow>::from_raw(
                            values,
                        )
                    })
                    .transpose()?;
                Ok((attempt, existing))
            },
            |(attempt, existing), stamp| {
                Ok(bind_session_decision(
                    stamp,
                    attempt,
                    &state.adapter,
                    &state.provider,
                    session_id,
                    model,
                    existing.as_ref(),
                ))
            },
        )
        .map(|_| ())
}

/// `_CancelChecker`: whether the run should stop. The job's flag counts at once; the attempt's own
/// flag is read from the database at most every two seconds, and stays set once seen.
fn cancel_requested(app: &mut App, state: &mut Recorder, job: &JobContext) -> bool {
    if state.cancelled || job.clone().cancel_requested() {
        return true;
    }
    if cancel_poll_due(state.last_poll.elapsed().as_secs_f64()) {
        state.last_poll = Instant::now();
        let attempt = state.attempt;
        state.cancelled = app
            .runtime
            .store
            .read(|session| session.attempt_by_id(attempt))
            .ok()
            .flatten()
            .is_some_and(|row| row.cancel_requested);
    }
    state.cancelled
}

/// `_finalize_failure`: ends the attempt before or while the run, and releases its worktrees.
fn finalize(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    end: &EarlyEnd,
) -> Result<(), WgError> {
    let max_attempts = execution_number(&app.config()?, "max_attempts_per_task", 3);
    app.runtime.decide(
        context,
        |session| {
            let attempt = session
                .attempt_by_id(attempt_id)?
                .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
            let world = session.task_world(Some(attempt.workspace_id), &[])?;
            let attempts = session.attempts_of_task(attempt.task_id, &[])?;
            Ok((attempt, world, attempts))
        },
        |(attempt, world, attempts), stamp| {
            let facts = EndFacts {
                world,
                attempts,
                max_attempts,
            };
            finalize_failure(&facts, stamp, attempt, end)
        },
    )?;
    worktree_release(app, context, attempt_id, false, false)?;
    Ok(())
}

/// The repositories the attempt asked for, from its config snapshot.
fn requested(snapshot: &Map<String, Json>) -> Option<Vec<PrepareRequest>> {
    let found: Vec<PrepareRequest> = snapshot
        .get("repositories")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(PrepareRequest {
                        repository_id: parse_id(item.get("repository_id")?.as_str()?).ok()?,
                        access: item.get("access")?.as_str()?.to_owned(),
                        target_ref: item
                            .get("target_ref")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        base: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (!found.is_empty()).then_some(found)
}

/// Writes the prompt below the artifacts directory and returns its row.
fn store_prompt(
    app: &mut App,
    context: &RequestContext,
    workspace: Id,
    task: Id,
    attempt: Id,
    prompt: &str,
) -> Result<wg_model::entity::Artifact, WgError> {
    let stamp = app.runtime.stamps.stamp(context)?;
    let id = stamp.id(7000);
    let name = "prompt.md";
    let path = stored_path(workspace, id, name);
    let file = PathBuf::from(app.paths.artifacts_dir().as_str()).join(&path);
    wg_runtime::write_atomic(&file, prompt.as_bytes())
        .map_err(|error| WgError::workgraph(format!("cannot write {}: {error}", file.display())))?;
    Ok(artifact_row(
        id,
        stamp.at,
        ArtifactSpec {
            workspace_id: workspace,
            task_id: Some(task),
            attempt_id: Some(attempt),
            kind: "prompt".to_owned(),
            name: name.to_owned(),
            media_type: "text/markdown".to_owned(),
            retention: "keep".to_owned(),
            metadata: Map::from_iter([("prompt_version".to_owned(), json!(PROMPT_VERSION))]),
        },
        path,
        prompt.as_bytes(),
    ))
}

/// `register_file` for the agent's log, when it wrote one.
fn register_log(
    app: &mut App,
    context: &RequestContext,
    workspace: Id,
    task: Id,
    attempt: Id,
    log: &std::path::Path,
) -> Result<Option<wg_model::entity::Artifact>, WgError> {
    let Ok(bytes) = std::fs::read(log) else {
        return Ok(None);
    };
    let root = PathBuf::from(app.paths.artifacts_dir().as_str());
    let Ok(relative) = log.strip_prefix(&root) else {
        return Ok(None);
    };
    let stamp = app.runtime.stamps.stamp(context)?;
    Ok(Some(artifact_row(
        stamp.id(7001),
        stamp.at,
        ArtifactSpec {
            workspace_id: workspace,
            task_id: Some(task),
            attempt_id: Some(attempt),
            kind: "agent_log".to_owned(),
            name: LOG_NAME.to_owned(),
            media_type: "text/plain".to_owned(),
            retention: "telemetry".to_owned(),
            metadata: Map::new(),
        },
        relative.to_string_lossy().replace('\\', "/"),
        &bytes,
    )))
}

/// `execute`: runs one queued attempt to its conclusion. The result is what the job reports.
pub fn execute(
    app: &mut App,
    host: &mut dyn AgentHost,
    collaborators: &mut dyn Collaborators,
    job: &mut JobContext,
) -> Result<Json, WgError> {
    let attempt_id = job
        .job
        .payload
        .get("attempt_id")
        .and_then(Json::as_str)
        .ok_or_else(|| missing_payload("attempt_id"))
        .and_then(parse_id)?;
    let context = system_context("runner")?.with_attempt(Some(attempt_id));
    let attempt = app
        .runtime
        .store
        .read(|session| session.attempt_by_id(attempt_id))?
        .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
    match execute_entry(attempt.status, attempt.cancel_requested) {
        ExecuteEntry::Skip(reason) => return Ok(json!({ "skipped": reason })),
        ExecuteEntry::CancelBeforeStart => {
            finalize(
                app,
                &context,
                attempt_id,
                &EarlyEnd::cancelled_before_start(),
            )?;
            return Ok(json!({"status": "cancelled"}));
        }
        ExecuteEntry::Proceed => {}
    }
    let (task_id, workspace_id) = (attempt.task_id, attempt.workspace_id);
    let slug = app.workspace_slug(&workspace_id.to_string())?;
    let agent_name = attempt.provider.clone();
    let snapshot = attempt.input_task_snapshot.clone();
    let config_snapshot = attempt.config_snapshot.clone();
    let effective = app.effective_config(Some(&slug))?;
    let provider = match require_available(host, &effective, &agent_name) {
        Ok(found) => found,
        Err(error)
            if matches!(
                error.code,
                ErrorCode::ProviderUnavailable | ErrorCode::NotFound
            ) =>
        {
            finalize(
                app,
                &context,
                attempt_id,
                &EarlyEnd::provider_unavailable(&error.message),
            )?;
            return Ok(json!({"status": "failed", "error": error.message}));
        }
        Err(error) => return Err(error),
    };

    app.runtime.decide(
        &context,
        |session| {
            let attempt = session
                .attempt_by_id(attempt_id)?
                .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
            let world = session.task_world(Some(workspace_id), &[])?;
            Ok((attempt, world))
        },
        |(fresh, world), stamp| preparing_decision(world, fresh, stamp),
    )?;
    let manifest = match worktree_prepare(
        app,
        &context,
        attempt_id,
        requested(&config_snapshot),
        None,
        &[],
    ) {
        Ok(found) => found,
        Err(error) => {
            finalize(
                app,
                &context,
                attempt_id,
                &EarlyEnd::preparation_failed(&error.message),
            )?;
            return Ok(json!({"status": "failed", "error": error.message}));
        }
    };

    let repositories: Vec<PromptRepo> = manifest
        .repositories
        .iter()
        .map(|entry| PromptRepo {
            repository: entry.repository.clone(),
            checkout_path: entry.checkout_path.clone(),
            access: entry.access.clone(),
            sha: entry.sha.clone(),
        })
        .collect();
    let prompt = build_prompt(
        &snapshot,
        &repositories,
        config_snapshot.get("instructions").and_then(Json::as_str),
    );
    let attempt_text = attempt_id.to_string();
    let runs_dir = PathBuf::from(app.paths.runs_dir().as_str()).join(&attempt_text);
    std::fs::create_dir_all(&runs_dir).map_err(|error| {
        WgError::workgraph(format!("cannot create {}: {error}", runs_dir.display()))
    })?;
    let log_path = PathBuf::from(app.paths.artifacts_dir().as_str())
        .join(workspace_id.to_string())
        .join("runs")
        .join(&attempt_text)
        .join(LOG_NAME);
    let workspace_dir = PathBuf::from(&manifest.workspace_dir);
    let checkouts: Vec<Checkout> = manifest
        .repositories
        .iter()
        .map(|entry| Checkout {
            path: PathBuf::from(&entry.checkout_path),
            read_write: entry.access == "read_write",
        })
        .collect();
    let placed = choose_workdir(&checkouts, &workspace_dir);
    let environment = environment(
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION"),
        &provider.detail,
        &placed.workdir.display().to_string(),
        &manifest.workspace_dir,
    );
    let artifact = store_prompt(app, &context, workspace_id, task_id, attempt_id, &prompt)?;
    app.runtime.decide(
        &context,
        |session| {
            session
                .attempt_by_id(attempt_id)?
                .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))
        },
        |fresh, stamp| running_decision(fresh, artifact.clone(), environment.clone(), stamp),
    )?;

    let manifest_path = workspace_dir.join("workgraph-manifest.json");
    let task_key = text(snapshot.get("key")).to_owned();
    let home = std::env::var("WORKGRAPH_HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .and_then(|_| {
            PathBuf::from(app.paths.config_dir().as_str())
                .parent()
                .map(|parent| parent.display().to_string())
        });
    let request = AgentRunRequest {
        attempt_id: attempt_text.clone(),
        task_id: task_id.to_string(),
        task_key: task_key.clone(),
        prompt,
        workdir: placed.workdir,
        manifest_path: manifest_path.clone(),
        manifest: manifest.to_json(),
        timeout_seconds: timeout_seconds(
            config_snapshot
                .get("timeout_seconds")
                .and_then(Json::as_f64),
        ),
        log_path: log_path.clone(),
        env: agent_env(
            &attempt_text,
            &task_id.to_string(),
            &task_key,
            home.as_deref(),
            &manifest_path.display().to_string(),
        ),
        model: config_snapshot
            .get("model")
            .and_then(Json::as_str)
            .map(str::to_owned),
        resume_session_id: None,
        additional_dirs: placed.additional_dirs,
    };

    let pending: Pending = Arc::default();
    let sink = Arc::clone(&pending);
    let started = Instant::now();
    let recorder = RefCell::new(Recorder {
        context: context.clone(),
        workspace: workspace_id,
        attempt: attempt_id,
        adapter: provider.adapter.to_owned(),
        provider: agent_name.clone(),
        count: 0,
        last_write: Instant::now(),
        last_poll: Instant::now(),
        cancelled: false,
    });
    let launched = {
        let shared = RefCell::new(&mut *app);
        let probe = job.clone();
        let cancel = || {
            let mut app = shared.borrow_mut();
            let mut state = recorder.borrow_mut();
            drain(&mut app, &mut state, &pending, false);
            cancel_requested(&mut app, &mut state, &probe)
        };
        host.launch(
            &agent_name,
            &provider.entry,
            &request,
            Box::new(move |event| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(event);
            }),
            Some(&cancel),
        )
    };
    let result = match launched {
        Ok(found) => found,
        Err(AgentError::Unavailable(message)) => {
            synthetic_result(&provider_unavailable_result(&message))
        }
        Err(error) => synthetic_result(&adapter_crash_result("AgentError", &error.to_string())),
    };
    let measured_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let mut state = recorder.into_inner();
    drain(app, &mut state, &pending, true);

    let log = register_log(app, &context, workspace_id, task_id, attempt_id, &log_path)?;
    let mut telemetry = result.telemetry.clone();
    telemetry.insert("measured_duration_ms".to_owned(), json!(measured_ms));
    telemetry.insert(
        "provider_reported_duration_ms".to_owned(),
        json!(result.provider_duration_ms),
    );
    telemetry.insert("num_turns".to_owned(), json!(result.num_turns));
    telemetry.insert(
        "agent_log_artifact_id".to_owned(),
        json!(log.as_ref().map(|row| row.id.to_string())),
    );
    telemetry.insert(
        "artifacts".to_owned(),
        Json::Array(result.artifacts.clone()),
    );
    telemetry.insert("progress_events".to_owned(), json!(state.count));
    let input = Concluding {
        report: AgentReport {
            status: result.status.as_str().to_owned(),
            summary: Some(result.summary.clone()),
            error: result.error.clone(),
            exit_code: result.exit_code.map(i64::from),
            duration_ms: Some(measured_ms),
            usage_fields: usage_fields(&result.usage, result.cost_usd, provider.adapter),
        },
        record: Some(RunRecord {
            session_id: result.session_id.clone(),
            model: result.model.clone(),
            telemetry,
            log,
        }),
    };
    let probe = job.clone();
    conclude_attempt(
        app,
        &context,
        collaborators,
        task_id,
        attempt_id,
        &input,
        &move || probe.clone().cancel_requested(),
    )
}
