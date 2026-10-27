//! The verification run a stack verification starts (`VerificationService.run`, restricted to
//! the shape stacks use: the parent task, a given revision manifest, one tier, no attempt).
//!
//! A run is two transactions around the check processes: [`prepare_run`] stores the manifest
//! and the running run, [`finalize_run`] stores each check's result and judges the run.

use serde_json::{Map, Value};
use wg_model::entity::{
    Artifact, JsonObject, RevisionManifest, VerificationCheckResult, VerificationConfig,
    VerificationRun,
};
use wg_model::event::{EventPayload, VerificationCompleted, VerificationStarted};
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_stacks::regression::{RunCheck, RunResult};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::Change;
use wg_types::{ErrorCode, Id, Permission, TaskStatus, VerificationStatus, WgError};
use wg_verify::{
    ApplicableCheck, CheckResult, CheckSpec, CheckTier, TaskScope, applicable_checks,
    checks_for_tier, run_outcome,
};

use super::chain::Chain;

/// The most characters of a check's output a result keeps.
const EXCERPT_CHARS: usize = 8000;

/// A run that has been prepared and not yet finalized.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub run_id: Id,
    pub run: VerificationRun,
    pub manifest_id: Id,
    pub task_id: Id,
    pub task_key: String,
    pub workspace_id: Id,
    pub entries: Vec<JsonObject>,
    pub checks: Vec<ApplicableCheck>,
    pub tier: Option<CheckTier>,
    /// The `stack` member merged into the run's events.
    pub stack: Option<Value>,
}

/// The checks `_prepare` selects for a task: the tier's, then the caller's filter.
pub fn selected_checks(
    world: &TaskWorld,
    configs: &[VerificationConfig],
    task_id: Id,
    tier: Option<CheckTier>,
    keep: &dyn Fn(&ApplicableCheck) -> bool,
) -> Result<Vec<ApplicableCheck>, WgError> {
    let task = world
        .tasks
        .task(task_id)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("task {task_id} not found")))?;
    let scope = TaskScope {
        workspace_id: task.workspace_id,
        task_id,
        project_ids: world
            .tasks
            .projects_of(task_id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: world
            .tasks
            .repositories_of(task_id)
            .map(|row| row.repository_id)
            .collect(),
    };
    let all = applicable_checks(configs, &scope)?;
    let checks: Vec<ApplicableCheck> = checks_for_tier(&all, tier)
        .into_iter()
        .filter(|check| keep(check))
        .cloned()
        .collect();
    if checks.is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            match tier {
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
    Ok(checks)
}

/// `_prepare`: the manifest and the running run, and `verification.started`.
#[allow(clippy::too_many_arguments)]
pub fn prepare_run(
    world: &TaskWorld,
    stamp: &Stamp,
    task_id: Id,
    checks: Vec<ApplicableCheck>,
    entries: Vec<JsonObject>,
    tier: Option<CheckTier>,
    stack: Option<Value>,
) -> Result<Decision<Prepared>, WgError> {
    stamp.require(&[Permission::VerifyRun])?;
    let task = world
        .tasks
        .task(task_id)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("task {task_id} not found")))?;
    let chain = Chain::start(world, stamp);
    let (manifest_id, chain) = chain.next_id();
    let (run_id, chain) = chain.next_id();
    let manifest = RevisionManifest {
        id: manifest_id,
        workspace_id: task.workspace_id,
        attempt_id: None,
        kind: "verification".to_owned(),
        entries: entries.iter().cloned().collect(),
        created_at: stamp.at,
    };
    let snapshot = checks.iter().map(|check| {
        let mut entry = Map::new();
        entry.insert(
            "config_id".to_owned(),
            Value::String(check.config_id.to_string()),
        );
        entry.insert(
            "config".to_owned(),
            Value::String(check.config_name.clone()),
        );
        entry.extend(check.spec.to_json());
        entry
    });
    let run = VerificationRun {
        id: run_id,
        workspace_id: task.workspace_id,
        task_id,
        attempt_id: None,
        manifest_id: Some(manifest_id),
        config_snapshot: snapshot.collect(),
        status: VerificationStatus::Running.as_str().to_owned(),
        required_passed: false,
        triggered_by: stamp.actor.to_string(),
        started_at: Some(stamp.at),
        finished_at: None,
        created_at: stamp.at,
    };
    let chain = chain
        .write(Write::insert(manifest))
        .write(Write::insert(run.clone()))
        .event(
            task.workspace_id,
            run_id,
            EventPayload::VerificationStarted(VerificationStarted {
                checks: Value::from(checks.len()),
                manifest_id: Some(manifest_id),
                task_id,
                task_key: task.key.to_string(),
                tier: tier.map(|tier| tier.as_str().to_owned()),
                stack: stack.clone(),
            }),
        );
    Ok(chain.finish(Prepared {
        run_id,
        run,
        manifest_id,
        task_id,
        task_key: task.key.to_string(),
        workspace_id: task.workspace_id,
        entries,
        checks,
        tier,
        stack,
    }))
}

/// A check's log file, registered as an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    /// The file name (`<check>.log`).
    pub name: String,
    /// The path below the artifacts directory, with `/` separators.
    pub path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub media_type: String,
}

/// What running one check gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Executed {
    pub spec: CheckSpec,
    pub status: VerificationStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub timed_out: bool,
    pub excerpt: String,
    pub signatures: Vec<String>,
    pub log: Option<LogFile>,
    pub repository_id: Option<Id>,
}

/// The last `limit` characters of `text`.
#[must_use]
pub fn last_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(limit)).collect()
}

/// A finished run: the judgement the stack verification reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub result: RunResult,
}

/// `_finalize`: the results, the run's verdict, `verification.completed`, and the parent's move
/// when it awaits verification.
pub fn finalize_run(
    world: &TaskWorld,
    stamp: &Stamp,
    prepared: &Prepared,
    executed: &[Executed],
) -> Result<Decision<Finished>, WgError> {
    let chain = Chain::start(world, stamp);
    let run = prepared.run.clone();
    let (chain, rows) = executed.iter().try_fold(
        (chain, Vec::<VerificationCheckResult>::new()),
        |(chain, rows), done| -> Result<_, WgError> {
            let (artifact, chain) = match &done.log {
                Some(log) => {
                    let (id, chain) = chain.next_id();
                    let artifact = Artifact {
                        id,
                        workspace_id: prepared.workspace_id,
                        task_id: Some(prepared.task_id),
                        attempt_id: None,
                        kind: "verification_log".to_owned(),
                        name: log.name.clone(),
                        media_type: log.media_type.clone(),
                        path: log.path.clone(),
                        size_bytes: log.size_bytes,
                        sha256: log.sha256.clone(),
                        retention: "telemetry".to_owned(),
                        purged_at: None,
                        metadata: [
                            ("check".to_owned(), Value::String(done.spec.name.clone())),
                            (
                                "run_id".to_owned(),
                                Value::String(prepared.run_id.to_string()),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        created_at: stamp.at,
                    };
                    (Some(id), chain.write(Write::insert(artifact)))
                }
                None => (None, chain),
            };
            let (row_id, chain) = chain.next_id();
            let row = VerificationCheckResult {
                id: row_id,
                run_id: prepared.run_id,
                name: done.spec.name.clone(),
                kind: done.spec.kind.as_str().to_owned(),
                required: done.spec.required,
                repository_id: done.repository_id,
                status: done.status.as_str().to_owned(),
                exit_code: done.exit_code,
                duration_ms: Some(done.duration_ms),
                timed_out: done.timed_out,
                log_artifact_id: artifact,
                output_excerpt: last_chars(&done.excerpt, EXCERPT_CHARS),
                failure_signatures: done.signatures.iter().cloned().collect(),
                created_at: stamp.at,
            };
            Ok((
                chain.write(Write::insert(row.clone())),
                rows.into_iter().chain(std::iter::once(row)).collect(),
            ))
        },
    )?;
    let outcome = run_outcome(
        &executed
            .iter()
            .map(|done| CheckResult {
                name: done.spec.name.clone(),
                required: done.spec.required,
                status: done.status,
            })
            .collect::<Vec<_>>(),
    );
    let chain = chain.write(Write::update(VerificationRun {
        status: outcome.status.as_str().to_owned(),
        required_passed: outcome.required_passed,
        finished_at: Some(stamp.at),
        ..run
    }));
    let (event_id, chain) = chain.event_with_id(
        prepared.workspace_id,
        prepared.run_id,
        EventPayload::VerificationCompleted(VerificationCompleted {
            failed_checks: Value::from(outcome.failed_checks.clone()),
            optional_failures: Value::Bool(outcome.optional_failures),
            required_passed: outcome.required_passed,
            status: outcome.status.as_str().to_owned(),
            task_id: prepared.task_id,
            task_key: prepared.task_key.clone(),
            stack: prepared.stack.clone(),
        }),
        None,
    );
    let fast_pass = wg_verify::is_fast_pass(prepared.tier, outcome.required_passed);
    let status = chain.task(prepared.task_id)?.status;
    let chain = if status == TaskStatus::AwaitingVerification && !fast_pass {
        let target = if outcome.required_passed {
            TaskStatus::AwaitingReview
        } else {
            TaskStatus::Failed
        };
        let reason = format!("verification {}", outcome.status.as_str());
        chain.task_to(
            prepared.task_id,
            target,
            Change {
                reason: Some(&reason),
                caused_by: Some(event_id),
                run_id: Some(Some(prepared.run_id)),
                ..Change::default()
            },
        )?
    } else {
        chain
    };
    let result = RunResult {
        id: prepared.run_id.to_string(),
        required_passed: outcome.required_passed,
        checks: rows
            .iter()
            .zip(executed)
            .map(|(row, done)| RunCheck {
                name: row.name.clone(),
                required: row.required,
                status: done.status,
                repository_id: row.repository_id.map(|id| id.to_string()),
                failure_signatures: row.failure_signatures.iter().cloned().collect(),
                output_excerpt: row.output_excerpt.clone(),
            })
            .collect(),
    };
    Ok(chain.finish(Finished { result }))
}
