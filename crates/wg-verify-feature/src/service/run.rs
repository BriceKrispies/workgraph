//! Shell: `verify run` (`VerificationService.run`): plans the run, records its start, executes
//! every check as a child process, records the results and, when the task became reviewable,
//! accepts it automatically.
//!
//! Each step reads what its decision needs, in the transaction of the step; the checks run between
//! the transactions, so a long check never holds the write lock. The decisions are
//! [`crate::spec::run`] and [`crate::spec::auto_accept`]; this module spawns the processes, reads
//! git heads and logs, and applies what the decisions return.

use std::path::PathBuf;

use wg_git::Git;
use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::{JsonObject, VerificationRun};
use wg_proc::{Cancel, base_environment, run_process};
use wg_procspec::{LineOptions, ProcessSpec};
use wg_runtime::{App, RequestContext, index_written, latest_succeeded};
use wg_store::Session;
use wg_types::{Id, Permission, WgError, parse_id};
use wg_verify::{
    ApplicableCheck, CheckTier, ExecutedCheck, ProcessOutcome, RunIdentity, check_environment,
    log_file_name, manifest_entry,
};

use crate::spec::auto_accept::{AutoAcceptWorld, auto_accept, enabled_in};
use crate::spec::run::{
    FinishWorld, LogFile, Plan, PrepareWorld, RunRequest, checkout_of, failure, filled, finish_run,
    io_failure, manifest_repositories, manifest_wanted, plan_run, read_log, repositories_wanted,
    start_failure, start_run, task_of,
};
use crate::spec::sql::stack_repository_hits;

/// The arguments of `verify run`.
#[derive(Debug, Clone, Copy)]
pub struct RunInput<'a> {
    pub workspace: Option<&'a str>,
    pub task: &'a str,
    pub attempt: Option<&'a str>,
    pub tier: Option<CheckTier>,
}

fn load_prepare(
    session: &mut Session<'_>,
    input: &RunInput<'_>,
    scope: Option<Id>,
) -> Result<PrepareWorld, WgError> {
    let world = session.task_world(scope, &[])?;
    let (task, workspace) = {
        let found = task_of(&world, scope, input.task)?;
        (found.id, found.workspace_id)
    };
    let attempt = match input.attempt.and_then(|text| parse_id(text).ok()) {
        Some(id) => session.attempt_by_id(id)?,
        None => None,
    };
    let source_manifest = match manifest_wanted(attempt.as_ref()) {
        Some(id) => session.manifest_by_id(id)?,
        None => None,
    };
    let checkouts = repositories_wanted(&world, task)
        .into_iter()
        .map(|repository| session.checkouts(repository))
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    Ok(PrepareWorld {
        configs: session.verification_configs(workspace)?,
        world,
        attempt,
        source_manifest,
        checkouts,
    })
}

/// Where one run keeps its manifest file and logs.
struct Places {
    run_dir: PathBuf,
    artifacts: PathBuf,
    workspace: Id,
    run: Id,
}

/// `_execute_check`: runs one check against the manifest and returns its result and its log.
fn execute(
    places: &Places,
    plan: &Plan,
    entries: &[JsonObject],
    check: &ApplicableCheck,
    cancel: Option<Cancel<'_>>,
) -> Result<(ExecutedCheck, Option<LogFile>), WgError> {
    let spec = &check.spec;
    std::fs::create_dir_all(&places.run_dir)
        .map_err(|error| io_failure("create", &places.run_dir, &error))?;
    let manifest_path = places.run_dir.join("manifest.json");
    let document = serde_json::Value::Array(
        entries
            .iter()
            .cloned()
            .map(serde_json::Value::Object)
            .collect(),
    );
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&document).map_err(|error| failure(error.to_string()))?,
    )
    .map_err(|error| io_failure("write", &manifest_path, &error))?;
    let target = match spec.repository.as_deref() {
        None => Some((places.run_dir.clone(), None)),
        Some(reference) => manifest_entry(entries, reference)
            .and_then(checkout_of)
            .map(|(path, repository)| (PathBuf::from(path), Some(repository))),
    };
    let Some((cwd, repository)) = target else {
        return Ok((ExecutedCheck::missing_repository(spec), None));
    };
    let identity = RunIdentity {
        task_id: plan.task_id,
        task_key: &plan.task_key,
        run_id: places.run,
        manifest_path: &manifest_path.to_string_lossy(),
    };
    let log_name = log_file_name(&spec.name);
    let log_path = places
        .artifacts
        .join(places.workspace.to_string())
        .join("verification")
        .join(places.run.to_string())
        .join(&log_name);
    let mut process = ProcessSpec::new(spec.command.clone());
    process.cwd = Some(cwd);
    process.env = Some(base_environment(
        check_environment(&identity, spec, entries),
        false,
    ));
    process.timeout_ms = Some((spec.timeout_seconds * 1000.0) as u64);
    let options = LineOptions {
        log_file: Some(log_path.clone()),
        ..LineOptions::default()
    };
    let result = match run_process(&process, options, cancel) {
        Ok(result) => result,
        Err(error) => {
            return match start_failure(&error) {
                Some(message) => Ok((ExecutedCheck::not_started(spec, repository, &message), None)),
                None => Err(failure(error.to_string())),
            };
        }
    };
    let relative = format!(
        "{}/verification/{}/{}",
        places.workspace, places.run, log_name
    );
    let written = std::fs::read(&log_path).ok();
    let (log, lines) = match written.as_deref() {
        Some(bytes) => {
            let (file, lines) = read_log(&relative, &log_name, bytes);
            (Some(file), lines)
        }
        None => (None, result.tail.clone()),
    };
    let outcome = ProcessOutcome {
        ok: result.ok(),
        cancelled: result.cancelled,
        timed_out: result.timed_out,
        exit_code: result.exit_code.map(i64::from),
    };
    let executed = ExecutedCheck::finished(
        spec,
        repository,
        &outcome,
        i64::try_from(result.duration_ms).unwrap_or(i64::MAX),
        &result.tail,
        &lines,
        log.is_some(),
    );
    Ok((executed, log))
}

fn load_finish(
    session: &mut Session<'_>,
    run_id: Id,
    attempt: Option<Id>,
) -> Result<FinishWorld, WgError> {
    let run = session
        .fetch::<VerificationRun>(&RowKey::VerificationRun(run_id))?
        .ok_or_else(|| WgError::not_found(format!("verification run {run_id} not found")))?;
    let attempt_row = match attempt {
        Some(id) => session.attempt_by_id(id)?,
        None => None,
    };
    let change_set = match attempt {
        Some(id) => match session.change_sets_of_attempt(id)?.into_iter().next() {
            Some(set) => {
                let items = session.change_set_items(set.id)?;
                Some((set, items))
            }
            None => None,
        },
        None => None,
    };
    Ok(FinishWorld {
        world: session.task_world(Some(run.workspace_id), &[])?,
        run,
        attempt: attempt_row,
        change_set,
    })
}

fn load_auto_accept(
    session: &mut Session<'_>,
    run_id: Id,
    task: Id,
    entries: &[JsonObject],
    enabled: bool,
) -> Result<AutoAcceptWorld, WgError> {
    let run = session
        .fetch::<VerificationRun>(&RowKey::VerificationRun(run_id))?
        .ok_or_else(|| WgError::not_found(format!("verification run {run_id} not found")))?;
    let manifest = match run.manifest_id {
        Some(id) => session.manifest_by_id(id)?,
        None => None,
    };
    let lands = !session
        .rows(&stack_repository_hits(&manifest_repositories(entries)))?
        .is_empty();
    Ok(AutoAcceptWorld {
        world: session.task_world(Some(run.workspace_id), &[])?,
        latest_run: session
            .latest_verification_run(task)?
            .map(|latest| latest.id),
        enabled,
        lands_through_stacks: lands,
        facts: wg_tasks::accept::AcceptFacts {
            manifest,
            attempt: latest_succeeded(session, task)?,
            run: Some(run.clone()),
        },
        run,
    })
}

/// Runs the checks that apply to a task against its current revisions. The reply is the
/// `VerificationRunOut` document; `cancel` is polled while a check runs.
pub fn run(
    app: &mut App,
    context: &RequestContext,
    input: &RunInput<'_>,
    cancel: Option<Cancel<'_>>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::VerifyRun)?;
    let scope = app.workspace_or_all(input.workspace)?;
    let request = RunRequest {
        scope,
        task: input.task,
        attempt: input.attempt,
        tier: input.tier,
    };
    let loaded = app
        .runtime
        .store
        .read(|session| load_prepare(session, input, scope))?;
    let plan = plan_run(&loaded, &request)?;
    // `fill_heads`: the head and branch of each checkout that is a repository.
    let entries: Vec<JsonObject> = plan
        .entries
        .iter()
        .map(|entry| {
            filled(entry, &|path| {
                let git = Git {
                    path: PathBuf::from(path),
                    timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
                };
                git.is_repository().ok().filter(|found| *found).map(|_| {
                    (
                        git.head().ok().flatten(),
                        git.current_branch().ok().flatten(),
                    )
                })
            })
        })
        .collect();
    let started = app
        .runtime
        .decide(
            context,
            |session| match plan.attempt_id {
                Some(id) => session.attempt_by_id(id),
                None => Ok(None),
            },
            |attempt, stamp| start_run(&plan, &entries, attempt.as_ref(), stamp),
        )?
        .reply;
    let places = Places {
        run_dir: PathBuf::from(app.paths.runs_dir().as_str())
            .join("verification")
            .join(started.run_id.to_string()),
        artifacts: PathBuf::from(app.paths.artifacts_dir().as_str()),
        workspace: plan.workspace_id,
        run: started.run_id,
    };
    let results = plan
        .checks
        .iter()
        .map(|check| execute(&places, &plan, &entries, check, cancel))
        .collect::<Result<Vec<_>, _>>()?;
    let finished = app
        .runtime
        .decide_then(
            context,
            |session| load_finish(session, started.run_id, plan.attempt_id),
            |world, stamp| finish_run(world, &plan, &entries, &results, stamp),
            index_written,
        )?
        .reply;
    if finished.reviewable {
        accept_after(app, context, &plan, &entries, started.run_id)?;
    }
    Ok(finished.out)
}

/// `_auto_accept`, after the run is finalised.
fn accept_after(
    app: &mut App,
    context: &RequestContext,
    plan: &Plan,
    entries: &[JsonObject],
    run: Id,
) -> Result<(), WgError> {
    let slug = app.workspace_slug(&plan.workspace_id.to_string())?;
    let enabled = enabled_in(&app.effective_config(Some(&slug))?);
    app.runtime.decide_then(
        context,
        |session| load_auto_accept(session, run, plan.task_id, entries, enabled),
        |world, stamp| auto_accept(world, stamp, plan.task_id, &plan.checks),
        index_written,
    )?;
    Ok(())
}
