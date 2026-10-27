//! Shell: the trial runner entry points `trial start` and `trial grade` (IF-20, C081, C083), the
//! `TrialRunner` of `src/workgraph/experiments/runner.py`. A trial's clone holds only the replay
//! base branch, with no remote, so the solution commit is not reachable by any ref in it. Grading
//! measures the clone's working tree against the base, runs the oracle and the protocol's gates
//! there, and records what was measured. Nothing is judged by a model.
//!
//! Each step that writes is its own transaction, as in Python: `start` records the start after
//! the clone is ready, and `grade` links the run, records the grade and, for a flagged failure,
//! classifies it, one decision each.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json, json};
use wg_experiments::escape::{self, ToolCall};
use wg_json::Value;
use wg_model::entity::{Experiment, ExperimentTrial, SubagentRun, Task};
use wg_model::py_repr_str;
use wg_registry::checkout::primary_checkout;
use wg_runtime::{App, RequestContext};
use wg_store::{Session, many};
use wg_types::{Id, Permission, WgError};
use wg_wire::dto::experiment_shapes::trial_start;

use crate::service::exec::{
    current_branch, default_branch as git_default_branch, environment, out, run_command,
};
use crate::service::replay::{template_inputs, worktree_changes};
use crate::service::trials::{experiment_of, load_trial, trial_classify, trial_show};
use crate::spec::runner::{
    self, BRANCH_PREFIX, BriefInput, Commands, FlagInput, GradeInput, RunLink, RunRow, Spec,
    spec_of, timeout_ms,
};
use crate::spec::runner_sql;

/// What `start` reads before it clones: the trial, its experiment and task, and where the
/// task's repository is checked out.
#[derive(Clone)]
struct StartWorld {
    trial: ExperimentTrial,
    experiment: Experiment,
    task: Task,
    spec: Spec,
    repository: String,
    checkout: PathBuf,
}

/// The first repository linked to the task, with its primary checkout (`_repository`).
fn repository_of(
    session: &mut Session<'_>,
    world: &wg_store::TaskWorld,
    task_id: Id,
) -> Result<
    (
        wg_model::entity::Repository,
        Vec<wg_model::entity::RepositoryCheckout>,
    ),
    WgError,
> {
    let first = world
        .tasks
        .repositories_of(task_id)
        .min_by_key(|link| (link.created_at, link.repository_id))
        .ok_or_else(|| WgError::validation("the trial's task has no linked repository"))?;
    let found = world
        .tasks
        .registry
        .repository(first.repository_id)
        .cloned()
        .ok_or_else(|| {
            WgError::not_found(format!("repository {} not found", first.repository_id))
        })?;
    let checkouts = session.checkouts(found.id)?;
    Ok((found, checkouts))
}

fn load_start(
    session: &mut Session<'_>,
    scope: Id,
    trial_ref: &str,
) -> Result<StartWorld, WgError> {
    let (trial, _) = load_trial(session, scope, trial_ref)?;
    runner::require_planned(&trial)?;
    let experiment = experiment_of(session, &trial)?;
    let world = session.task_world(Some(scope), &[])?;
    let task = world
        .tasks
        .task(trial.task_id)
        .cloned()
        .ok_or_else(|| WgError::not_found(format!("task {} not found", trial.task_id)))?;
    let tags: Vec<String> = world.tasks.tags_of(task.id).map(str::to_owned).collect();
    let spec = spec_of(&task, &tags)?;
    let (repository, checkouts) = repository_of(session, &world, task.id)?;
    let checkout = primary_checkout(&checkouts, true)
        .map(|checkout| PathBuf::from(&checkout.path))
        .ok_or_else(|| {
            WgError::validation(format!(
                "repository {} has no available checkout",
                py_repr_str(&repository.name)
            ))
        })?;
    Ok(StartWorld {
        trial,
        experiment,
        task,
        spec,
        repository: repository.name,
        checkout,
    })
}

/// `_commit_message`: the subject and body of a commit.
fn commit_message(checkout: &Path, sha: &str) -> Result<(String, String), WgError> {
    let subject = out(checkout, &["log", "-1", "--format=%s", sha])?
        .trim()
        .to_owned();
    let body = out(checkout, &["log", "-1", "--format=%b", sha])?
        .trim_end_matches('\n')
        .to_owned();
    Ok((subject, body))
}

/// `_default_branch`: the default branch, or the checked-out one when no default can be read.
fn default_branch(checkout: &Path) -> Result<String, WgError> {
    let branch = match git_default_branch(checkout)? {
        Some(branch) => Some(branch),
        None => current_branch(checkout)?,
    };
    branch.ok_or_else(|| runner::no_default_branch(&checkout.display().to_string()))
}

/// The oracle and gates of a trial, with the template inputs read at `base_sha` in `clone`.
fn commands_at(
    protocol: &Map<String, Json>,
    spec: &Spec,
    clone: &Path,
    base_sha: &str,
    test_paths: &[String],
) -> Result<(Commands, Commands), WgError> {
    let inputs = runner::oracle_template(protocol, spec)
        .map(|_| template_inputs(clone, base_sha, test_paths))
        .transpose()?;
    runner::commands(protocol, spec, inputs.as_ref())
}

/// What the clone produced.
struct Prepared {
    base_sha: String,
    brief: String,
}

/// Clones the replay base into `dest`, runs the setup and renders the brief.
fn prepare(
    world: &StartWorld,
    protocol: &Map<String, Json>,
    dest: &Path,
    branch: &str,
) -> Result<Prepared, WgError> {
    let spec = &world.spec;
    let (subject, body) = match &spec.commit {
        None => (world.task.title.clone(), world.task.description.clone()),
        Some(commit) => commit_message(&world.checkout, commit)?,
    };
    let base_ref = match &spec.base_ref {
        Some(base) => base.clone(),
        None => default_branch(&world.checkout)?,
    };
    let env = environment(&runner::protocol_env(protocol));
    let shown = dest.display().to_string();
    // --single-branch with --branch <base> makes the clone hold only the base; the remote is then
    // removed so no other branch or tag of the source repository is reachable by ref from the
    // clone. "isolated" (the default) copies only the base branch's objects; "shared" keeps
    // alternates to the source object store, so solution objects stay readable by SHA.
    out(
        &world.checkout,
        &[
            "clone",
            runner::clone_mode(protocol),
            "--single-branch",
            "--no-tags",
            "--branch",
            &base_ref,
            &world.checkout.display().to_string(),
            &shown,
        ],
    )?;
    out(dest, &["checkout", "-b", branch])?;
    out(dest, &["remote", "remove", "origin"])?;
    for command in runner::setup_commands(protocol) {
        let argv = runner::setup_argv(&command, &shown);
        let record = run_command(&argv, dest, None, &env);
        if runner::record_exit(&record) != 0 {
            return Err(runner::setup_failure(&argv, &record));
        }
    }
    let base_sha = out(dest, &["rev-parse", "HEAD"])?.trim().to_owned();
    let test_paths = if spec.commit.is_none() {
        runner::declared_test_paths(&spec.files, &crate::service::exec::config_for(dest))
    } else {
        spec.test_paths.clone()
    };
    let (oracle, gates) = commands_at(protocol, spec, dest, &base_sha, &test_paths)?;
    let brief = runner::render_brief(
        protocol,
        &BriefInput {
            task_key: &world.task.key.to_string(),
            trial_id: world.trial.id,
            repository: &world.repository,
            worktree: &shown,
            branch,
            subject: &subject,
            body: &body,
            test_paths: &test_paths,
            oracle: &oracle,
            gates: &gates,
            spec,
        },
    )?;
    Ok(Prepared { base_sha, brief })
}

/// `TrialRunner.start`: clones the replay base into a fresh trial directory, runs the
/// protocol's setup, and renders the brief.
///
/// # Errors
///
/// Returns the `NotFound` of an unknown trial, the conflicts and validation failures of a trial
/// that cannot start, and the `GitError` of a failed clone.
pub fn trial_start_run(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    trial_ref: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    let world = app.runtime.query(
        |session| load_start(session, scope, trial_ref),
        |world| Ok(world.clone()),
    )?;
    let id = world.trial.id.to_string();
    let short: String = id.chars().take(8).collect();
    let branch = format!("{BRANCH_PREFIX}{short}");
    let dest = PathBuf::from(app.paths.worktrees_dir().as_str())
        .join("trials")
        .join(&short);
    if dest.exists() {
        return Err(WgError::conflict(format!(
            "trial directory {} already exists",
            dest.display()
        ))
        .detail("path", dest.display().to_string()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| WgError::workgraph(format!("{}: {error}", parent.display())))?;
    }
    let started = start_in(app, context, scope, &world, &dest, &branch);
    if started.is_err() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    let (reply, prepared) = started?;
    Ok(trial_start(
        reply,
        &dest.display().to_string(),
        &branch,
        &prepared.base_sha,
        &prepared.brief,
    ))
}

fn start_in(
    app: &mut App,
    context: &RequestContext,
    scope: Id,
    world: &StartWorld,
    dest: &Path,
    branch: &str,
) -> Result<(Value, Prepared), WgError> {
    let prepared = prepare(world, &world.experiment.protocol, dest, branch)?;
    let shown = dest.display().to_string();
    let trial_ref = world.trial.id.to_string();
    let reply = app
        .runtime
        .decide_then(
            context,
            |session| {
                let (trial, task_key) = load_trial(session, scope, &trial_ref)?;
                let experiment = experiment_of(session, &trial)?;
                Ok((trial, task_key, experiment))
            },
            |(trial, task_key, experiment): &(ExperimentTrial, String, Experiment), stamp| {
                runner::start(
                    experiment,
                    trial,
                    task_key,
                    &prepared.base_sha,
                    &shown,
                    stamp,
                )
            },
            |_, _| Ok(()),
        )?
        .reply;
    Ok((reply, prepared))
}

/// What `grade` reads before it measures.
#[derive(Clone)]
struct GradeWorld {
    trial: ExperimentTrial,
    experiment: Experiment,
    task: Task,
    tags: Vec<String>,
    run: Option<RunRow>,
    checkouts: Vec<String>,
}

/// `_latest_run`: the most recent subagent run whose telemetry names this trial.
fn latest_run(session: &mut Session<'_>, trial_id: Id) -> Result<Option<RunRow>, WgError> {
    let runs: Vec<SubagentRun> = many(session, &runner_sql::runs_naming_trial(trial_id))?;
    let needle = trial_id.to_string();
    Ok(runs
        .iter()
        .find(|run| {
            run.telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.get("trial_ids"))
                .and_then(Json::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(needle.as_str())))
        })
        .map(RunRow::of))
}

fn load_grade(
    session: &mut Session<'_>,
    scope: Id,
    trial_ref: &str,
) -> Result<GradeWorld, WgError> {
    let (trial, _) = load_trial(session, scope, trial_ref)?;
    runner::require_gradable(&trial)?;
    let experiment = experiment_of(session, &trial)?;
    let world = session.task_world(Some(scope), &[])?;
    let task = world
        .tasks
        .task(trial.task_id)
        .cloned()
        .ok_or_else(|| WgError::not_found(format!("task {} not found", trial.task_id)))?;
    let run = match trial.subagent_run_id {
        None => latest_run(session, trial.id)?,
        Some(run_id) => wg_store::one::<SubagentRun>(session, &runner_sql::run_by_id(run_id))?
            .as_ref()
            .map(RunRow::of),
    };
    let checkouts = match repository_of(session, &world, task.id) {
        Ok((_, checkouts)) => {
            let mut user: Vec<_> = checkouts
                .into_iter()
                .filter(|checkout| checkout.kind != "managed_worktree")
                .collect();
            user.sort_by_key(|checkout| (!checkout.is_primary, checkout.created_at));
            user.into_iter().map(|checkout| checkout.path).collect()
        }
        Err(_) => Vec::new(),
    };
    let tags: Vec<String> = world.tasks.tags_of(task.id).map(str::to_owned).collect();
    Ok(GradeWorld {
        trial,
        experiment,
        task,
        tags,
        run,
        checkouts,
    })
}

/// `_escapes`: tool calls in the linked run's transcript that name a path outside the clone.
fn escapes(
    run: Option<&RunRow>,
    roots: &[String],
    worktree: &Path,
) -> (&'static str, Vec<escape::Escape>) {
    let Some(run) = run else {
        return ("no_run", Vec::new());
    };
    let Some(path) = run
        .transcript
        .as_deref()
        .map(Path::new)
        .filter(|path| path.is_file())
    else {
        return ("no_transcript", Vec::new());
    };
    let lines: Vec<String> = std::fs::read(path)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let calls: Vec<ToolCall> = wg_telemetry::transcript::tool_calls(Some(&lines))
        .into_iter()
        .map(|call| ToolCall {
            name: call.name,
            input: call.input,
            tool_use_id: call.tool_use_id,
        })
        .collect();
    let forbidden = runner::source_roots(&worktree.display().to_string(), roots);
    ("checked", escape::find_escapes(&calls, &forbidden))
}

/// Reloads a trial, its task key and its experiment inside a write transaction.
fn reload(
    session: &mut Session<'_>,
    scope: Id,
    trial_ref: &str,
) -> Result<(ExperimentTrial, String, Experiment), WgError> {
    let (trial, task_key) = load_trial(session, scope, trial_ref)?;
    let experiment = experiment_of(session, &trial)?;
    Ok((trial, task_key, experiment))
}

fn current_trial(app: &mut App, scope: Id, trial_ref: &str) -> Result<ExperimentTrial, WgError> {
    app.runtime.query(
        |session| load_trial(session, scope, trial_ref),
        |(trial, _)| Ok(trial.clone()),
    )
}

/// `TrialRunner.grade`: measures a started trial's clone and records the outcome; a flagged
/// failure is classed as gaming.
///
/// # Errors
///
/// Returns the `NotFound` of an unknown trial, the conflicts and validation failures of a trial
/// that cannot be graded, and the errors of the measurement.
pub fn trial_grade_run(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    trial_ref: &str,
    timeout: f64,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    let world = app.runtime.query(
        |session| load_grade(session, scope, trial_ref),
        |world| Ok(world.clone()),
    )?;
    let trial_id = world.trial.id.to_string();
    let worktree = PathBuf::from(world.trial.worktree_path.clone().unwrap_or_default());
    if !worktree.is_dir() {
        return Err(WgError::validation(format!(
            "trial worktree {} is missing",
            worktree.display()
        ))
        .detail("path", worktree.display().to_string()));
    }
    let base_sha = world.trial.base_ref.clone().unwrap_or_default();
    let protocol = &world.experiment.protocol;
    let spec = spec_of(&world.task, &world.tags)?;
    let (oracle, gates) = commands_at(protocol, &spec, &worktree, &base_sha, &spec.test_paths)?;

    if let (None, Some(run)) = (world.trial.subagent_run_id, world.run.as_ref()) {
        let link = RunLink {
            attempt_id: run.attempt_id,
            subagent_run_id: Some(run.id),
            actual_model: run.model.clone(),
        };
        app.runtime.decide_then(
            context,
            |session| reload(session, scope, &trial_id),
            |(trial, task_key, experiment): &(ExperimentTrial, String, Experiment), stamp| {
                runner::link_run(experiment, trial, task_key, &link, stamp)
            },
            |_, _| Ok(()),
        )?;
    }
    let trial = current_trial(app, scope, &trial_id)?;

    let changes = worktree_changes(&worktree, &base_sha)?;
    let (escape_check, found) = escapes(world.run.as_ref(), &world.checkouts, &worktree);
    let flags = runner::grade_flags(&FlagInput {
        tests_modified: &changes.tests_modified,
        stubs: &changes.stubs,
        leaf: spec.leaf,
        declared_files: &spec.files,
        changed_paths: &changes.changed_paths,
        escapes: &found,
    });

    let env = environment(&runner::protocol_env(protocol));
    let limit = Some(timeout_ms(timeout));
    let oracle_records: Vec<Json> = oracle
        .iter()
        .map(|argv| run_command(argv, &worktree, limit, &env))
        .collect();
    let gate_records: Vec<Json> = gates
        .iter()
        .map(|argv| run_command(argv, &worktree, limit, &env))
        .collect();
    let head_sha = out(&worktree, &["rev-parse", "HEAD"])?.trim().to_owned();
    let passed = oracle_records
        .iter()
        .chain(&gate_records)
        .all(|record| runner::record_exit(record) == 0)
        && flags.is_empty();
    let detail = json!({
        "base_sha": base_sha,
        "head_sha": head_sha,
        "run": runner::run_detail(&trial),
        "oracle": oracle_records,
        "gates": gate_records,
        "flags": flags,
        "escape_check": escape_check,
    });
    let detail = match detail {
        Json::Object(map) => map,
        _ => Map::new(),
    };
    let features = match crate::spec::replay::features_json(&changes.features) {
        Json::Object(map) => Some(map),
        _ => None,
    };
    let outcome = if passed { "pass" } else { "fail" };
    app.runtime.decide_then(
        context,
        |session| reload(session, scope, &trial_id),
        |(trial, task_key, experiment): &(ExperimentTrial, String, Experiment), stamp| {
            runner::record_grade(
                experiment,
                trial,
                task_key,
                GradeInput {
                    outcome,
                    detail: detail.clone(),
                    features: features.clone(),
                },
                stamp,
            )
        },
        |_, _| Ok(()),
    )?;
    let graded = current_trial(app, scope, &trial_id)?;
    if !passed && !flags.is_empty() && graded.status != "invalid" {
        trial_classify(
            app,
            context,
            workspace,
            &trial_id,
            "gaming",
            "mechanical",
            Some(&runner::flag_summary(&flags)),
        )?;
    }
    trial_show(app, context, workspace, &trial_id)
}
