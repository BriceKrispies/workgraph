//! Pure: the decisions of `verify run` (`VerificationService.run`, `_prepare` and `_finalize`).
//!
//! A run has three decisions around the checks the shell executes: [`plan_run`] chooses the checks
//! and the revision entries (nothing is written); [`start_run`] writes the manifest, the run row
//! and the `verification.started` event; [`finish_run`] records every check result and log
//! artifact, settles the run, the change set, the attempt and the task, and replies with the
//! `VerificationRunOut` document.

use serde_json::Value as Json;
use wg_exec::attempt::{FinishPlan, finish_plan};
use wg_exec::change_set::{verified_item_status, verified_status};
use wg_json::Value;
use wg_model::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, JsonObject, RepositoryCheckout,
    RevisionManifest, VerificationCheckResult, VerificationConfig, VerificationRun,
};
use wg_model::event::{
    AttemptFinished, AttemptStatusChanged, EventDraft, EventPayload, VerificationCompleted,
    VerificationStarted,
};
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_registry::checkout::primary_checkout;
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::lifecycle::check_attempt_transition;
use wg_tasks::transition::{Change, transition};
use wg_types::{
    AttemptStatus, ChangeSetItemStatus, ErrorCode, Id, TaskStatus, VerificationStatus, WgError,
    parse_id,
};
use wg_verify::{
    ApplicableCheck, CheckResult, CheckTier, ExecutedCheck, TaskScope, applicable_checks,
    checks_for_tier, is_fast_pass, run_outcome,
};
use wg_wire::dto::verification::run_from_model;

/// The longest error text an attempt keeps.
const ATTEMPT_ERROR_CHARS: usize = 20_000;

/// The longest error text an attempt event carries.
const EVENT_ERROR_CHARS: usize = 1000;

/// What `verify run` reads before it decides anything.
#[derive(Debug, Clone, Default)]
pub struct PrepareWorld {
    pub world: TaskWorld,
    /// The verification configurations of the task's workspace.
    pub configs: Vec<VerificationConfig>,
    /// The attempt the request names, when it exists.
    pub attempt: Option<AgentAttempt>,
    /// The attempt's outcome manifest, else its baseline manifest.
    pub source_manifest: Option<RevisionManifest>,
    /// The checkouts of every repository the task is linked to.
    pub checkouts: Vec<RepositoryCheckout>,
}

/// The request of `verify run`.
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    pub scope: Option<Id>,
    pub task: &'a str,
    pub attempt: Option<&'a str>,
    pub tier: Option<CheckTier>,
}

/// The checks a run executes and the revisions it executes them against.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub task_id: Id,
    pub task_key: String,
    pub workspace_id: Id,
    pub attempt_id: Option<Id>,
    pub tier: Option<CheckTier>,
    pub checks: Vec<ApplicableCheck>,
    /// One entry per repository; the shell fills the `sha` and `ref` of each from its checkout.
    pub entries: Vec<JsonObject>,
}

fn missing_attempt(reference: &str) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("attempt {} not found", wg_model::py_repr_str(reference)),
    )
    .with_detail("ref", Json::String(reference.to_owned()))
}

/// The task a reference names, found in the loaded world.
pub fn task_of<'a>(
    world: &'a TaskWorld,
    scope: Option<Id>,
    reference: &str,
) -> Result<&'a wg_model::Task, WgError> {
    resolve::task(&world.tasks, scope, reference)
}

/// `manifest_from_checkouts`: the current head of each repository the task is linked to.
fn checkout_entries(world: &PrepareWorld, task_id: Id) -> Vec<JsonObject> {
    let tasks = &world.world.tasks;
    tasks
        .repositories_of(task_id)
        .filter_map(|link| {
            let repository = tasks.registry.repository(link.repository_id)?;
            let own: Vec<RepositoryCheckout> = world
                .checkouts
                .iter()
                .filter(|checkout| checkout.repository_id == repository.id)
                .cloned()
                .collect();
            let path =
                primary_checkout(&own, true).map(|checkout| Json::String(checkout.path.clone()));
            Some(
                [
                    ("repository_id", Json::String(repository.id.to_string())),
                    ("repository", Json::String(repository.name.clone())),
                    ("checkout_path", path.unwrap_or(Json::Null)),
                    ("sha", Json::Null),
                    ("ref", Json::Null),
                    ("access", Json::String(link.access.clone())),
                ]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
            )
        })
        .collect()
}

/// `_prepare`, the part that reads: the checks of the tier and the revision entries.
pub fn plan_run(world: &PrepareWorld, request: &RunRequest<'_>) -> Result<Plan, WgError> {
    let tasks = &world.world.tasks;
    let task = task_of(&world.world, request.scope, request.task)?;
    let scope = TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: tasks
            .projects_of(task.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: tasks
            .repositories_of(task.id)
            .map(|row| row.repository_id)
            .collect(),
    };
    let all = applicable_checks(&world.configs, &scope)?;
    let checks: Vec<ApplicableCheck> = checks_for_tier(&all, request.tier)
        .into_iter()
        .cloned()
        .collect();
    if checks.is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            match request.tier {
                Some(tier) => format!(
                    "no {} verification checks are configured for {}",
                    tier.as_str(),
                    task.key
                ),
                None => format!(
                    "no verification checks are configured for {}; record manual acceptance instead",
                    task.key
                ),
            },
        ));
    }
    let attempt = match request.attempt {
        Some(reference) => Some(
            world
                .attempt
                .as_ref()
                .filter(|found| parse_id(reference).is_ok_and(|id| id == found.id))
                .ok_or_else(|| missing_attempt(reference))?,
        ),
        None => None,
    };
    let entries = match world.source_manifest.as_ref() {
        Some(manifest) if attempt.is_some() => manifest.entries.iter().cloned().collect(),
        _ => checkout_entries(world, task.id),
    };
    Ok(Plan {
        task_id: task.id,
        task_key: task.key.to_string(),
        workspace_id: task.workspace_id,
        attempt_id: attempt.map(|found| found.id),
        tier: request.tier,
        checks,
        entries,
    })
}

/// The manifest an attempt's entries come from: its outcome, else its baseline.
pub fn manifest_wanted(attempt: Option<&AgentAttempt>) -> Option<Id> {
    attempt.and_then(|found| found.outcome_manifest_id.or(found.baseline_manifest_id))
}

/// The repositories a task is linked to.
pub fn repositories_wanted(world: &TaskWorld, task: Id) -> Vec<Id> {
    world
        .tasks
        .repositories_of(task)
        .map(|link| link.repository_id)
        .collect()
}

/// The repositories the entries of a manifest name.
pub fn manifest_repositories(entries: &[JsonObject]) -> Vec<Id> {
    entries
        .iter()
        .filter_map(|entry| entry.get("repository_id").and_then(Json::as_str))
        .filter_map(|text| parse_id(text).ok())
        .collect()
}

/// The checkout path and repository of a manifest entry a check can run in.
pub fn checkout_of(entry: &JsonObject) -> Option<(String, Id)> {
    let path = entry
        .get("checkout_path")
        .and_then(Json::as_str)
        .filter(|path| !path.is_empty())?;
    let repository = entry
        .get("repository_id")
        .and_then(Json::as_str)
        .and_then(|text| parse_id(text).ok())?;
    Some((path.to_owned(), repository))
}

/// The checkout path of an entry whose head is not known yet.
pub fn head_wanted(entry: &JsonObject) -> Option<&str> {
    match entry.get("sha") {
        None | Some(Json::Null) => entry
            .get("checkout_path")
            .and_then(Json::as_str)
            .filter(|path| !path.is_empty()),
        Some(_) => None,
    }
}

/// `fill_heads` for one entry: the head and branch of its checkout.
pub fn with_head(entry: &JsonObject, sha: Option<String>, branch: Option<String>) -> JsonObject {
    let text = |value: Option<String>| value.map_or(Json::Null, Json::String);
    entry
        .iter()
        .map(|(key, value)| match key.as_str() {
            "sha" => (key.clone(), text(sha.clone())),
            "ref" => (key.clone(), text(branch.clone())),
            _ => (key.clone(), value.clone()),
        })
        .collect()
}

/// The head and branch of a checkout.
pub type Head = (Option<String>, Option<String>);

/// `fill_heads` for one entry: `probe` reads the head and branch of the checkout at a path, and
/// answers `None` when the path is not a repository.
pub fn filled(entry: &JsonObject, probe: &dyn Fn(&str) -> Option<Head>) -> JsonObject {
    match head_wanted(entry).and_then(probe) {
        Some((sha, branch)) => with_head(entry, sha, branch),
        None => entry.clone(),
    }
}

/// A failure of the runner itself, outside any check.
pub fn failure(message: String) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, message)
}

/// A file the runner could not create or write.
pub fn io_failure(what: &str, path: &std::path::Path, error: &std::io::Error) -> WgError {
    failure(format!("cannot {what} {}: {error}", path.display()))
}

/// A row a decision needs that is not there.
pub fn not_found(what: &str, id: Id) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("{what} {id} not found"))
}

/// What a started run is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    pub run_id: Id,
    pub manifest_id: Id,
}

fn snapshot(check: &ApplicableCheck) -> JsonObject {
    [
        (
            "config_id".to_owned(),
            Json::String(check.config_id.to_string()),
        ),
        ("config".to_owned(), Json::String(check.config_name.clone())),
    ]
    .into_iter()
    .chain(check.spec.to_json())
    .collect()
}

/// `_prepare`, the part that writes: the verification manifest, the run row (`running`), the
/// attempt moved to `verifying`, and the `verification.started` event.
pub fn start_run(
    plan: &Plan,
    entries: &[JsonObject],
    attempt: Option<&AgentAttempt>,
    stamp: &Stamp,
) -> Result<Decision<Started>, WgError> {
    let ([manifest_id, run_id, moved_id, started_id], _) = stamp.ids().take::<4>();
    let manifest = RevisionManifest {
        id: manifest_id,
        workspace_id: plan.workspace_id,
        attempt_id: plan.attempt_id,
        kind: "verification".to_owned(),
        entries: entries.iter().cloned().collect(),
        created_at: stamp.at,
    };
    let run = VerificationRun {
        id: run_id,
        workspace_id: plan.workspace_id,
        task_id: plan.task_id,
        attempt_id: plan.attempt_id,
        manifest_id: Some(manifest_id),
        config_snapshot: plan.checks.iter().map(snapshot).collect(),
        status: VerificationStatus::Running.as_str().to_owned(),
        required_passed: false,
        triggered_by: stamp.actor.to_string(),
        started_at: Some(stamp.at),
        finished_at: None,
        created_at: stamp.at,
    };
    let verifying = match attempt.filter(|found| found.status == AttemptStatus::Running) {
        Some(found) => {
            check_attempt_transition(found.status, AttemptStatus::Verifying)?;
            Decision::new(())
                .write(Write::update(AgentAttempt {
                    status: AttemptStatus::Verifying,
                    updated_at: stamp.at,
                    ..found.clone()
                }))
                .event(
                    EventDraft::new(
                        stamp,
                        moved_id,
                        Some(found.workspace_id),
                        found.id,
                        EventPayload::AttemptStatusChanged(AttemptStatusChanged {
                            from: found.status,
                            to: AttemptStatus::Verifying,
                            task_id: found.task_id,
                            extra: JsonObject::new(),
                        }),
                    )
                    .with_attempt(Some(found.id)),
                )
        }
        None => Decision::new(()),
    };
    let started = EventDraft::new(
        stamp,
        started_id,
        Some(plan.workspace_id),
        run_id,
        EventPayload::VerificationStarted(VerificationStarted {
            checks: Json::from(plan.checks.len()),
            manifest_id: Some(manifest_id),
            task_id: plan.task_id,
            task_key: plan.task_key.clone(),
            tier: plan.tier.map(|tier| tier.as_str().to_owned()),
            stack: None,
        }),
    )
    .with_attempt(plan.attempt_id);
    Ok(Decision::new(())
        .write(Write::insert(manifest))
        .write(Write::insert(run))
        .then(verifying)
        .event(started)
        .with_reply(Started {
            run_id,
            manifest_id,
        }))
}

/// A check's log, as the artifact row records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    pub name: String,
    /// The path below the artifacts directory, with `/` separators.
    pub path: String,
    pub size: i64,
    pub sha256: String,
}

/// The log of a check from the bytes it wrote: the artifact facts, and the lines the failure
/// signatures are read from.
pub fn read_log(relative: &str, name: &str, bytes: &[u8]) -> (LogFile, Vec<String>) {
    let file = LogFile {
        name: name.to_owned(),
        path: relative.to_owned(),
        size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        sha256: wg_hash::sha256_hex(bytes),
    };
    let lines = String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect();
    (file, lines)
}

/// What `_finalize` reads: the run as started, and the rows its outcome moves.
#[derive(Debug, Clone)]
pub struct FinishWorld {
    pub world: TaskWorld,
    pub run: VerificationRun,
    pub attempt: Option<AgentAttempt>,
    pub change_set: Option<(ChangeSet, Vec<ChangeSetItem>)>,
}

/// The reply of [`finish_run`].
#[derive(Debug, Clone)]
pub struct Finished {
    /// The `VerificationRunOut` document.
    pub out: Value,
    /// The task reached `awaiting_review` and may be accepted automatically.
    pub reviewable: bool,
}

/// `_apply_change_set_status`: committed items become verified or failed, and the change set
/// follows.
fn settle_change_set(
    change_set: &(ChangeSet, Vec<ChangeSetItem>),
    passed: bool,
    stamp: &Stamp,
) -> Decision<()> {
    let (set, items) = change_set;
    let settled: Vec<ChangeSetItem> = items
        .iter()
        .map(|item| ChangeSetItem {
            status: verified_item_status(item.status, passed),
            updated_at: if item.status == ChangeSetItemStatus::Committed {
                stamp.at
            } else {
                item.updated_at
            },
            ..item.clone()
        })
        .collect();
    let statuses: Vec<ChangeSetItemStatus> = settled.iter().map(|item| item.status).collect();
    let rows = settled
        .into_iter()
        .zip(items)
        .filter(|(after, before)| after.status != before.status)
        .fold(Decision::new(()), |decision, (after, _)| {
            decision.write(Write::update(after))
        });
    match verified_status(&statuses, passed) {
        Some(status) => rows.write(Write::update(ChangeSet {
            status,
            updated_at: stamp.at,
            ..set.clone()
        })),
        None => rows,
    }
}

/// `finish_attempt` for a verifying attempt: succeeded when the required checks passed, else
/// failed with the names of the checks that did not pass.
fn finish_attempt(
    attempt: &AgentAttempt,
    stamp: &Stamp,
    event_id: Id,
    caused_by: Id,
    failed_checks: &[String],
    passed: bool,
) -> Result<Decision<()>, WgError> {
    let status = if passed {
        AttemptStatus::Succeeded
    } else {
        AttemptStatus::Failed
    };
    let FinishPlan::Apply { .. } = finish_plan(attempt.status, status)? else {
        return Ok(Decision::new(()));
    };
    let exit_reason = if passed {
        "verification_passed"
    } else {
        "verification_failed"
    };
    let error = (!passed).then(|| format!("verification failed: {}", failed_checks.join(", ")));
    let duration = attempt.started_at.map(|started| {
        i64::try_from(
            (stamp.at.to_offset_datetime() - started.to_offset_datetime()).whole_milliseconds(),
        )
        .unwrap_or(i64::MAX)
        .max(0)
    });
    let finished = AgentAttempt {
        status,
        finished_at: Some(stamp.at),
        exit_reason: Some(exit_reason.to_owned()),
        exit_code: None,
        error: error
            .as_ref()
            .map(|text| text.chars().take(ATTEMPT_ERROR_CHARS).collect())
            .or_else(|| attempt.error.clone()),
        duration_ms: duration.or(attempt.duration_ms),
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let body = AttemptFinished {
        task_id: attempt.task_id,
        from: attempt.status,
        status,
        exit_reason: Some(exit_reason.to_owned()),
        exit_code: None,
        duration_ms: finished.duration_ms,
        error: error
            .map(|text| text.chars().take(EVENT_ERROR_CHARS).collect::<String>())
            .filter(|text| !text.is_empty()),
    };
    let payload = if passed {
        EventPayload::AttemptSucceeded(body)
    } else {
        EventPayload::AttemptFailed(body)
    };
    Ok(Decision::new(()).write(Write::update(finished)).event(
        EventDraft::new(
            stamp,
            event_id,
            Some(attempt.workspace_id),
            attempt.id,
            payload,
        )
        .with_attempt(Some(attempt.id))
        .caused_by(caused_by),
    ))
}

/// One check's rows: its result and, when it logged, the artifact that holds the log.
fn check_rows(
    run: &VerificationRun,
    plan: &Plan,
    result: &ExecutedCheck,
    log: Option<&LogFile>,
    stamp: &Stamp,
    ids: [Id; 2],
    position: usize,
) -> (VerificationCheckResult, Option<Artifact>) {
    let artifact = log.map(|file| Artifact {
        id: ids[1],
        workspace_id: plan.workspace_id,
        task_id: Some(plan.task_id),
        attempt_id: plan.attempt_id,
        kind: "verification_log".to_owned(),
        name: file.name.clone(),
        media_type: "text/plain".to_owned(),
        path: file.path.clone(),
        size_bytes: file.size,
        sha256: file.sha256.clone(),
        retention: "telemetry".to_owned(),
        purged_at: None,
        metadata: [
            ("check".to_owned(), Json::String(result.spec.name.clone())),
            ("run_id".to_owned(), Json::String(run.id.to_string())),
        ]
        .into_iter()
        .collect(),
        created_at: stamp.at,
    });
    // Results order by creation time; a result a microsecond after the one before it keeps the
    // order the checks ran in.
    let offset = i64::try_from(position).unwrap_or(i64::MAX);
    let created_at = wg_types::Timestamp::from_offset_datetime(
        stamp.at.to_offset_datetime() + time::Duration::microseconds(offset),
    )
    .unwrap_or(stamp.at);
    let row = VerificationCheckResult {
        id: ids[0],
        run_id: run.id,
        name: result.spec.name.clone(),
        kind: result.spec.kind.as_str().to_owned(),
        required: result.spec.required,
        repository_id: result.repository_id,
        status: result.status.as_str().to_owned(),
        exit_code: result.exit_code,
        duration_ms: Some(result.duration_ms),
        timed_out: result.timed_out,
        log_artifact_id: artifact.as_ref().map(|row| row.id),
        output_excerpt: result.excerpt.clone(),
        failure_signatures: result.signatures.iter().cloned().collect(),
        created_at,
    };
    (row, artifact)
}

/// `_finalize`: records the results of the checks and settles the run.
pub fn finish_run(
    loaded: &FinishWorld,
    plan: &Plan,
    entries: &[JsonObject],
    results: &[(ExecutedCheck, Option<LogFile>)],
    stamp: &Stamp,
) -> Result<Decision<Finished>, WgError> {
    let run = &loaded.run;
    let judged: Vec<CheckResult> = results
        .iter()
        .map(|(result, _)| CheckResult {
            name: result.spec.name.clone(),
            required: result.spec.required,
            status: result.status,
        })
        .collect();
    let outcome = run_outcome(&judged);
    let (row_ids, ids) = results
        .iter()
        .fold((Vec::new(), stamp.ids()), |(rows, ids), _| {
            let ([result_id, artifact_id], next) = ids.take::<2>();
            (
                rows.into_iter()
                    .chain(std::iter::once([result_id, artifact_id]))
                    .collect(),
                next,
            )
        });
    let rows: Vec<(VerificationCheckResult, Option<Artifact>)> = results
        .iter()
        .zip(&row_ids)
        .enumerate()
        .map(|(position, ((result, log), pair))| {
            check_rows(run, plan, result, log.as_ref(), stamp, *pair, position)
        })
        .collect();
    let recorded = rows
        .iter()
        .fold(Decision::new(()), |decision, (row, artifact)| {
            let decision = match artifact {
                Some(found) => decision.write(Write::insert(found.clone())),
                None => decision,
            };
            decision.write(Write::insert(row.clone()))
        });
    let settled_run = VerificationRun {
        status: outcome.status.as_str().to_owned(),
        required_passed: outcome.required_passed,
        finished_at: Some(stamp.at),
        ..run.clone()
    };
    let (completed_id, ids) = ids.next();
    let completed = EventDraft::new(
        stamp,
        completed_id,
        Some(plan.workspace_id),
        run.id,
        EventPayload::VerificationCompleted(VerificationCompleted {
            failed_checks: Json::from(outcome.failed_checks.clone()),
            optional_failures: Json::Bool(outcome.optional_failures),
            required_passed: outcome.required_passed,
            status: outcome.status.as_str().to_owned(),
            task_id: plan.task_id,
            task_key: plan.task_key.clone(),
            stack: None,
        }),
    )
    .with_attempt(plan.attempt_id);
    let fast_pass = is_fast_pass(plan.tier, outcome.required_passed);
    let change_set = match loaded
        .change_set
        .as_ref()
        .filter(|_| plan.attempt_id.is_some() && !fast_pass)
    {
        Some(found) => settle_change_set(found, outcome.required_passed, stamp),
        None => Decision::new(()),
    };
    let (attempt_event, ids) = ids.next();
    let attempt = match loaded
        .attempt
        .as_ref()
        .filter(|found| found.status == AttemptStatus::Verifying)
    {
        Some(found) => finish_attempt(
            found,
            stamp,
            attempt_event,
            completed_id,
            &outcome.failed_checks,
            outcome.required_passed,
        )?,
        None => Decision::new(()),
    };
    let task = task_in(&loaded.world, plan.task_id)?;
    let moves = task.status == TaskStatus::AwaitingVerification && !fast_pass;
    let moved = if moves {
        let target = if outcome.required_passed {
            TaskStatus::AwaitingReview
        } else {
            TaskStatus::Failed
        };
        let applied = transition(
            &loaded.world.tasks,
            &loaded.world.stacks,
            stamp,
            ids,
            plan.task_id,
            target,
            Change {
                reason: Some(&format!("verification {}", outcome.status.as_str())),
                caused_by: Some(completed_id),
                run_id: Some(Some(run.id)),
                ..Change::default()
            },
        )?;
        applied.decision
    } else {
        Decision::new(())
    };
    let checks: Vec<VerificationCheckResult> = rows.into_iter().map(|(row, _)| row).collect();
    let out = run_from_model(&settled_run, entries, &checks);
    Ok(Decision::new(())
        .write(Write::update(settled_run))
        .then(recorded)
        .event(completed)
        .then(change_set)
        .then(attempt)
        .then(moved)
        .with_reply(Finished {
            out,
            reviewable: moves && outcome.required_passed,
        }))
}

/// The fields of a `verify` job's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPayload {
    pub task_id: String,
    pub attempt_id: Option<String>,
    pub tier: Option<String>,
}

/// Reads a `verify` job's payload. A missing `task_id` is the `KeyError` Python raises.
pub fn job_payload(payload: &JsonObject) -> Result<JobPayload, String> {
    let text = |key: &str| payload.get(key).and_then(Json::as_str).map(str::to_owned);
    Ok(JobPayload {
        task_id: text("task_id").ok_or_else(|| "KeyError: 'task_id'".to_owned())?,
        attempt_id: text("attempt_id"),
        tier: text("tier"),
    })
}

/// `_ctx(job, component)`: the system context under the job's correlation id, caused by the event
/// that queued the job.
pub fn job_context(
    base: wg_runtimespec::RequestContext,
    correlation: Option<wg_types::Correlation>,
    causation: Option<Id>,
) -> wg_runtimespec::RequestContext {
    wg_runtimespec::RequestContext {
        correlation: correlation.unwrap_or(base.correlation.clone()),
        causation,
        ..base
    }
}

/// The result a `verify` job records: the id and status of the run.
pub fn job_result(run: &Value) -> JsonObject {
    ["id", "status"]
        .into_iter()
        .map(|key| {
            let text = wg_clispec::args::get(run, key).map(wg_clispec::args::python);
            (
                if key == "id" { "run_id" } else { key }.to_owned(),
                text.map_or(Json::Null, Json::String),
            )
        })
        .collect()
}

/// The message of a check whose process could not be started, in the words `run_process` uses.
/// `None` for a failure after the process started, which is not a result of the check.
pub fn start_failure(error: &wg_procspec::ProcError) -> Option<String> {
    use wg_procspec::ProcError;
    match error {
        ProcError::EmptyArgv | ProcError::InvalidExecutable | ProcError::Spawn { .. } => {
            Some(error.to_string())
        }
        ProcError::NotFound(name) if name.contains(['/', '\\']) => {
            Some(format!("executable {name} does not exist"))
        }
        ProcError::NotFound(name) => Some(format!(
            "executable {} was not found on PATH",
            wg_model::py_repr_str(name)
        )),
        ProcError::LogFile { .. } | ProcError::Wait(_) => None,
    }
}

fn task_in(world: &TaskWorld, task: Id) -> Result<&wg_model::Task, WgError> {
    world
        .tasks
        .task(task)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("task {task} not found")))
}
