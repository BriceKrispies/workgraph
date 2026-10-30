//! Pure: the trial runner of `src/workgraph/experiments/runner.py` (IF-20, C081, C083) and the
//! `ExperimentService` methods it drives, `start_trial`, `record_grade` and `link_run`.
//!
//! The shell in [`crate::service::runner`] reads the rows, runs git and the oracle commands, and
//! applies what these functions decide. Everything here is a function of its arguments: the
//! task's spec, the oracle and gate commands, the grade flags, the trial updates and the views.

use serde_json::{Map, Value as Json, json};
use wg_experiments::escape::Escape;
use wg_experiments::replay::{self, ReplayConfig};
use wg_experiments::trials as lifecycle;
use wg_experiments::{contract, escape};
use wg_iface::cli::args::{get, items, python, s};
use wg_json::Value;
use wg_model::entity::{Experiment, ExperimentTrial, JsonObject};
use wg_model::event::{
    EventPayload, ExperimentTrialGraded, ExperimentTrialInvalidated, ExperimentTrialLinked,
    ExperimentTrialStarted,
};
use wg_model::{Decision, EventDraft, Row, Stamp, Write};
use wg_render::Console;
use wg_types::{FailureCause, FailureCauseSource, Id, TrialOutcome, TrialStatus, WgError};
use wg_wire::dto::trial::from_model;

use crate::spec::trial::{choice, parse_status, require_running};
use wg_types::pyval::{int_or, text};

/// The prefix of a trial's branch (`BRANCH_PREFIX`).
pub const BRANCH_PREFIX: &str = "wg/trial/";
/// How many trailing characters of a command's output a record keeps (`OUTPUT_TAIL`).
pub const OUTPUT_TAIL: usize = 2000;

/// A list of argv lists (`Commands`).
pub type Commands = Vec<Vec<String>>;

/// What a task's trial is run and graded against: a replay commit, or a leaf's declared
/// contract (`_Spec`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub leaf: bool,
    pub base_ref: Option<String>,
    pub commit: Option<String>,
    pub test_paths: Vec<String>,
    pub oracle: Commands,
    pub gates: Commands,
    pub files: Vec<String>,
    pub out_of_scope: Vec<String>,
    pub acceptance: Vec<(String, String)>,
}

/// Python's `str()` of a JSON value.
fn py_text(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        Json::Null => "None".to_owned(),
        Json::Bool(true) => "True".to_owned(),
        Json::Bool(false) => "False".to_owned(),
        other => other.to_string(),
    }
}

fn texts(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .map(py_text)
        .collect()
}

/// `_argv_tuples`: a list of argv lists, every part as text.
fn argv_lists(value: Option<&Json>) -> Commands {
    value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .map(|command| texts(Some(command)))
        .collect()
}

fn missing_key(key: &str) -> WgError {
    WgError::workgraph(format!("'{key}'"))
}

/// `_task_spec`: the spec of a replay task, or of a leaf whose `metadata.contract` is complete.
/// Anything else is refused.
///
/// # Errors
///
/// Returns the `ValidationFailed` of a task that is neither a replay task nor a complete leaf.
pub fn task_spec(
    key: &str,
    tags: &[String],
    criteria: &[String],
    metadata: &JsonObject,
) -> Result<Spec, WgError> {
    if let Some(meta) = metadata.get("replay").and_then(Json::as_object) {
        if !meta.get("sha").is_some_and(Json::is_string) {
            return Err(WgError::validation(format!(
                "task {key} is not a replay task"
            )));
        }
        let field = |name: &str| meta.get(name).ok_or_else(|| missing_key(name));
        return Ok(Spec {
            leaf: false,
            base_ref: Some(py_text(field("base_ref")?)),
            commit: Some(py_text(field("sha")?)),
            test_paths: texts(Some(field("test_paths")?)),
            oracle: argv_lists(meta.get("oracle")),
            gates: Vec::new(),
            files: Vec::new(),
            out_of_scope: Vec::new(),
            acceptance: Vec::new(),
        });
    }
    let problems = contract::contract_problems(tags, criteria, &Json::Object(metadata.clone()));
    if !problems.missing.is_empty() || !problems.not_delegable.is_empty() {
        let listed: Vec<String> = problems
            .not_delegable
            .iter()
            .cloned()
            .chain(
                problems
                    .missing
                    .iter()
                    .map(|name| format!("missing {name}")),
            )
            .collect();
        return Err(WgError::validation(format!(
            "task {key} has no replay metadata and cannot run as a leaf: {}",
            listed.join("; ")
        ))
        .detail("task", key)
        .detail("missing", problems.missing)
        .detail("not_delegable", problems.not_delegable));
    }
    let contract = metadata
        .get("contract")
        .and_then(Json::as_object)
        .ok_or_else(|| missing_key("contract"))?;
    let field = |name: &str| contract.get(name).ok_or_else(|| missing_key(name));
    let checks = texts(Some(field("checks")?));
    Ok(Spec {
        leaf: true,
        base_ref: contract
            .get("base_ref")
            .filter(|value| !value.is_null())
            .map(py_text),
        commit: None,
        test_paths: Vec::new(),
        oracle: argv_lists(Some(field("oracle")?)),
        gates: argv_lists(Some(field("gates")?)),
        files: texts(Some(field("files")?)),
        out_of_scope: texts(Some(field("out_of_scope")?)),
        acceptance: criteria.iter().cloned().zip(checks).collect(),
    })
}

/// `_declared_test_paths`: the declared files that match a test glob of the repository.
#[must_use]
pub fn declared_test_paths(files: &[String], config: &ReplayConfig) -> Vec<String> {
    files
        .iter()
        .filter(|path| {
            config
                .test_globs
                .iter()
                .any(|glob| replay::match_glob(path, glob))
        })
        .cloned()
        .collect()
}

/// The protocol's oracle template, when this task's oracle comes from it (replay tasks only).
#[must_use]
pub fn oracle_template(protocol: &JsonObject, spec: &Spec) -> Option<Vec<String>> {
    protocol
        .get("oracle_template")
        .filter(|template| !template.is_null() && !spec.leaf)
        .map(|template| texts(Some(template)))
}

/// `_commands`: the oracle and gate commands of a trial, shared by start and grade so they
/// cannot disagree. `inputs` is `(existing test paths, packages)` at the base, read by the shell
/// when [`oracle_template`] applies. The oracle must exist, or the trial cannot be run or graded.
///
/// # Errors
///
/// Returns the `ValidationFailed` of a trial with no oracle command.
pub fn commands(
    protocol: &JsonObject,
    spec: &Spec,
    inputs: Option<&(Vec<String>, Vec<String>)>,
) -> Result<(Commands, Commands), WgError> {
    let oracle: Commands = match (oracle_template(protocol, spec), inputs) {
        (Some(template), Some((existing, packages))) => {
            if existing.is_empty() {
                Vec::new()
            } else {
                vec![replay::expand_oracle_template(
                    &template, existing, packages,
                )]
            }
        }
        _ => spec.oracle.clone(),
    };
    if oracle.is_empty() {
        return Err(WgError::validation(
            "the trial has no oracle command, so it cannot be run or graded",
        ));
    }
    let gates = spec
        .gates
        .iter()
        .cloned()
        .chain(argv_lists(protocol.get("gates")))
        .collect();
    Ok((oracle, gates))
}

/// The files of a tree listing (`ls-tree -r -z --name-only`).
#[must_use]
pub fn tree_files(output: &str) -> std::collections::BTreeSet<String> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The protocol's `env` object as pairs; a non-text value is shown as Python shows it.
#[must_use]
pub fn protocol_env(protocol: &JsonObject) -> Vec<(String, String)> {
    protocol
        .get("env")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), py_text(value)))
        .collect()
}

/// The last [`OUTPUT_TAIL`] characters of a command's output.
#[must_use]
pub fn output_tail(output: &str) -> String {
    let count = output.chars().count();
    output
        .chars()
        .skip(count.saturating_sub(OUTPUT_TAIL))
        .collect()
}

/// One command record of `_run_command`: `argv`, `exit`, `duration_ms`, `output_tail`.
#[must_use]
pub fn command_record(argv: &[String], exit: i64, duration_ms: u128, output: &str) -> Json {
    json!({
        "argv": argv,
        "exit": exit,
        "duration_ms": u64::try_from(duration_ms).unwrap_or(u64::MAX),
        "output_tail": output_tail(output),
    })
}

/// The `exit` of a command record.
#[must_use]
pub fn record_exit(record: &Json) -> i64 {
    int_or(record.get("exit"), -1)
}

/// The text a failed setup command reports: `setup command <argv> exited <code>: <tail>`.
///
/// # Errors
///
/// Always returns the `ValidationFailed` the setup failure raises.
pub fn setup_failure(argv: &[String], record: &Json) -> WgError {
    WgError::validation(format!(
        "setup command {} exited {}: {}",
        wg_experiments::briefs::shlex_join(argv),
        record_exit(record),
        text(record.get("output_tail"))
    ))
    .detail("command", argv)
    .detail("exit", record_exit(record))
}

/// The protocol's `setup` commands: argv lists whose parts are text.
#[must_use]
pub fn setup_commands(protocol: &JsonObject) -> Commands {
    argv_lists(protocol.get("setup"))
}

/// The `{worktree}` placeholder of a setup command replaced by the clone's path.
#[must_use]
pub fn setup_argv(command: &[String], dest: &str) -> Vec<String> {
    command
        .iter()
        .map(|part| part.replace("{worktree}", dest))
        .collect()
}

/// The clone argument of the protocol: `--no-local` when isolated (the default), else `--shared`.
#[must_use]
pub fn clone_mode(protocol: &JsonObject) -> &'static str {
    match protocol.get("clone").map(py_text).as_deref() {
        None | Some("isolated") => "--no-local",
        Some(_) => "--shared",
    }
}

/// What the brief of a trial is rendered from.
pub struct BriefInput<'a> {
    pub task_key: &'a str,
    pub trial_id: Id,
    pub repository: &'a str,
    pub worktree: &'a str,
    pub branch: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    pub test_paths: &'a [String],
    pub oracle: &'a Commands,
    pub gates: &'a Commands,
    pub spec: &'a Spec,
}

/// `briefs.render` of the protocol's `brief_template`.
///
/// # Errors
///
/// Returns the `ValidationFailed` of an unknown template.
pub fn render_brief(protocol: &JsonObject, input: &BriefInput<'_>) -> Result<String, WgError> {
    let template = protocol
        .get("brief_template")
        .map(py_text)
        .ok_or_else(|| missing_key("brief_template"))?;
    let context = wg_experiments::briefs::BriefContext {
        task_key: input.task_key.to_owned(),
        trial_id: input.trial_id.to_string(),
        repository: input.repository.to_owned(),
        worktree: input.worktree.to_owned(),
        branch: input.branch.to_owned(),
        subject: input.subject.to_owned(),
        body: input.body.to_owned(),
        test_paths: input.test_paths.to_vec(),
        oracle: input.oracle.clone(),
        gates: input.gates.clone(),
        acceptance: input.spec.acceptance.clone(),
        files: input.spec.files.clone(),
        out_of_scope: input.spec.out_of_scope.clone(),
    };
    wg_experiments::briefs::render(&template, &context).map_err(|error| {
        WgError::validation(error.message())
            .detail("template", template)
            .detail("known", wg_experiments::briefs::TEMPLATE_IDS)
    })
}

/// The flags of a graded trial: what the measurement found wrong with the clone.
pub struct FlagInput<'a> {
    pub tests_modified: &'a [String],
    pub stubs: &'a [(String, String)],
    pub leaf: bool,
    pub declared_files: &'a [String],
    pub changed_paths: &'a [String],
    pub escapes: &'a [Escape],
}

/// The `flags` of `grade`, keys in the order the Python source adds them.
#[must_use]
pub fn grade_flags(input: &FlagInput<'_>) -> Map<String, Json> {
    let undeclared: Vec<&String> = input
        .changed_paths
        .iter()
        .filter(|path| !input.declared_files.contains(path))
        .collect();
    [
        (
            "tests_modified",
            (!input.tests_modified.is_empty()).then(|| json!(input.tests_modified)),
        ),
        (
            "stubs",
            (!input.stubs.is_empty()).then(|| {
                Json::Array(
                    input
                        .stubs
                        .iter()
                        .map(|(path, line)| json!({"path": path, "line": line}))
                        .collect(),
                )
            }),
        ),
        (
            "outside_declared_files",
            (input.leaf && !undeclared.is_empty()).then(|| json!(undeclared)),
        ),
        (
            "escaped_clone",
            (!input.escapes.is_empty()).then(|| {
                Json::Array(
                    input
                        .escapes
                        .iter()
                        .map(|e| {
                            json!({
                                "tool": e.tool,
                                "tool_use_id": e.tool_use_id,
                                "root": e.root,
                                "excerpt": e.excerpt,
                            })
                        })
                        .collect(),
                )
            }),
        ),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|v| (name.to_owned(), v)))
    .collect()
}

fn strings_of(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

fn sorted_paths(value: Option<&Json>, key: &str) -> Vec<String> {
    let mut found: Vec<String> = value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get(key).and_then(Json::as_str).map(str::to_owned))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// `_flag_summary`: the notes of the `gaming` classification.
#[must_use]
pub fn flag_summary(flags: &Map<String, Json>) -> String {
    [
        flags
            .get("tests_modified")
            .map(|listed| format!("tests modified: {}", strings_of(Some(listed)).join(", "))),
        flags.get("stubs").map(|listed| {
            format!(
                "stubs in: {}",
                sorted_paths(Some(listed), "path").join(", ")
            )
        }),
        flags.get("escaped_clone").map(|listed| {
            format!(
                "escaped clone via: {}",
                sorted_paths(Some(listed), "root").join(", ")
            )
        }),
        flags.get("outside_declared_files").map(|listed| {
            format!(
                "outside declared files: {}",
                strings_of(Some(listed)).join(", ")
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("; ")
}

/// The forbidden roots of `_source_roots`: the checkout paths of the task's repository that are
/// not managed worktrees, primary first, leaving out the clone itself and any directory that
/// contains it. `checkouts` is the paths of the user checkouts, primary first.
#[must_use]
pub fn source_roots(clone: &str, checkouts: &[String]) -> Vec<String> {
    let clone_text = escape::normalize_path_text(clone)
        .trim_end_matches('/')
        .to_owned();
    checkouts
        .iter()
        .filter(|path| {
            let root_text = escape::normalize_path_text(path)
                .trim_end_matches('/')
                .to_owned();
            !(clone_text == root_text || clone_text.starts_with(&format!("{root_text}/")))
        })
        .cloned()
        .collect()
}

/// The conflict of an operation refused on the trial's status.
fn status_conflict(trial: &ExperimentTrial, requirement: &str) -> WgError {
    WgError::conflict(format!(
        "trial {} is {}; {requirement}",
        trial.id, trial.status
    ))
    .detail("trial", trial.id.to_string())
    .detail("status", &trial.status)
}

/// The runner's own status check at the head of `start`.
///
/// # Errors
///
/// Returns the `ConflictError` of a trial that is not planned.
pub fn require_planned(trial: &ExperimentTrial) -> Result<(), WgError> {
    if trial.status == TrialStatus::Planned.as_str() {
        Ok(())
    } else {
        Err(status_conflict(trial, "only a planned trial can start"))
    }
}

/// The runner's own status check at the head of `grade`.
///
/// # Errors
///
/// Returns the `ConflictError` of a trial that is neither running nor invalid, and the
/// `ValidationFailed` of a trial that was never started.
pub fn require_gradable(trial: &ExperimentTrial) -> Result<(), WgError> {
    let gradable = [TrialStatus::Running, TrialStatus::Invalid]
        .iter()
        .any(|status| status.as_str() == trial.status);
    if !gradable {
        return Err(status_conflict(
            trial,
            "only a running or invalid trial is graded",
        ));
    }
    if trial.worktree_path.is_none() || trial.base_ref.is_none() {
        return Err(WgError::validation(format!(
            "trial {} has no worktree; start it before grading it",
            trial.id
        )));
    }
    Ok(())
}

/// `ExperimentService.start_trial` over the loaded rows.
///
/// # Errors
///
/// Returns the `ConflictError` of an experiment that is not running or a trial that is not
/// planned.
pub fn start(
    experiment: &Experiment,
    trial: &ExperimentTrial,
    task_key: &str,
    base_ref: &str,
    worktree: &str,
    stamp: &Stamp,
) -> Result<Decision<Value>, WgError> {
    require_running(experiment)?;
    let next = lifecycle::start(parse_status(&trial.status)?)
        .map_err(|rejected| status_conflict(trial, rejected.requirement))?;
    let started = ExperimentTrial {
        status: next.as_str().to_owned(),
        started_at: Some(stamp.at),
        base_ref: Some(base_ref.to_owned()),
        worktree_path: Some(worktree.to_owned()),
        ..trial.clone()
    };
    let event = EventDraft::new(
        stamp,
        stamp.ids().next().0,
        Some(experiment.workspace_id),
        trial.id,
        EventPayload::ExperimentTrialStarted(ExperimentTrialStarted {
            base_ref: Some(base_ref.to_owned()),
            experiment: experiment.slug.clone(),
            worktree_path: Some(worktree.to_owned()),
        }),
    );
    Ok(Decision::new(from_model(&started, task_key))
        .write(Write::update(Row::ExperimentTrial(started)))
        .event(event))
}

/// What a grade stores besides its outcome.
pub struct GradeInput<'a> {
    pub outcome: &'a str,
    pub detail: JsonObject,
    pub features: Option<JsonObject>,
}

/// `ExperimentService.record_grade` over the loaded rows. An invalid trial keeps its status but
/// still stores its outcome.
///
/// # Errors
///
/// Returns the `ConflictError` of an experiment that is not running or a trial that is neither
/// running nor invalid, and the `ValidationFailed` of an unknown outcome.
pub fn record_grade(
    experiment: &Experiment,
    trial: &ExperimentTrial,
    task_key: &str,
    input: GradeInput<'_>,
    stamp: &Stamp,
) -> Result<Decision<Value>, WgError> {
    require_running(experiment)?;
    let measured = choice(
        input.outcome,
        "outcome",
        TrialOutcome::ALL,
        TrialOutcome::as_str,
    )?;
    let next = lifecycle::grade(parse_status(&trial.status)?)
        .map_err(|rejected| status_conflict(trial, rejected.requirement))?;
    let features = input.features.map_or_else(
        || trial.features.clone(),
        |actual| {
            let mut merged = trial.features.clone();
            merged.insert("actual".to_owned(), Json::Object(actual));
            merged
        },
    );
    let graded = ExperimentTrial {
        status: next.as_str().to_owned(),
        outcome: Some(measured.as_str().to_owned()),
        outcome_detail: input.detail,
        features,
        graded_at: Some(stamp.at),
        ..trial.clone()
    };
    let event = EventDraft::new(
        stamp,
        stamp.ids().next().0,
        Some(experiment.workspace_id),
        trial.id,
        EventPayload::ExperimentTrialGraded(ExperimentTrialGraded {
            experiment: experiment.slug.clone(),
            outcome: Some(measured.as_str().to_owned()),
            status: graded.status.clone(),
        }),
    );
    Ok(Decision::new(from_model(&graded, task_key))
        .write(Write::update(Row::ExperimentTrial(graded)))
        .event(event))
}

/// The run a trial is linked to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLink {
    pub attempt_id: Option<Id>,
    pub subagent_run_id: Option<Id>,
    pub actual_model: Option<String>,
}

/// `ExperimentService.link_run`: attaches the attempt or subagent run that executed a trial and
/// the model that actually ran. A run on another model than the arm pins makes the trial invalid
/// (`wrong_model`, decided mechanically), which keeps it out of every fit.
///
/// # Errors
///
/// Returns the `ConflictError` of an experiment that is not running.
pub fn link_run(
    experiment: &Experiment,
    trial: &ExperimentTrial,
    task_key: &str,
    link: &RunLink,
    stamp: &Stamp,
) -> Result<Decision<Value>, WgError> {
    require_running(experiment)?;
    let linked = ExperimentTrial {
        attempt_id: link.attempt_id.or(trial.attempt_id),
        subagent_run_id: link.subagent_run_id.or(trial.subagent_run_id),
        actual_model: link
            .actual_model
            .clone()
            .or_else(|| trial.actual_model.clone()),
        ..trial.clone()
    };
    let wrong_model = link
        .actual_model
        .as_deref()
        .is_some_and(|model| model != trial.requested_model)
        && trial.status != TrialStatus::Invalid.as_str();
    let next = if wrong_model {
        ExperimentTrial {
            status: TrialStatus::Invalid.as_str().to_owned(),
            failure_cause: Some(FailureCause::WrongModel.as_str().to_owned()),
            failure_cause_source: Some(FailureCauseSource::Mechanical.as_str().to_owned()),
            ..linked
        }
    } else {
        linked
    };
    let ids = stamp.ids();
    let (linked_id, ids) = ids.next();
    let (invalid_id, _) = ids.next();
    let workspace = Some(experiment.workspace_id);
    let linked_event = EventDraft::new(
        stamp,
        linked_id,
        workspace,
        trial.id,
        EventPayload::ExperimentTrialLinked(ExperimentTrialLinked {
            actual_model: link.actual_model.clone(),
            attempt_id: link.attempt_id,
            experiment: experiment.slug.clone(),
            requested_model: trial.requested_model.clone(),
            subagent_run_id: link.subagent_run_id,
        }),
    );
    let decision = Decision::new(from_model(&next, task_key))
        .write(Write::update(Row::ExperimentTrial(next)))
        .event(linked_event);
    Ok(if wrong_model {
        decision.event(EventDraft::new(
            stamp,
            invalid_id,
            workspace,
            trial.id,
            EventPayload::ExperimentTrialInvalidated(ExperimentTrialInvalidated {
                actual_model: link.actual_model.clone(),
                experiment: experiment.slug.clone(),
                failure_cause: Some(FailureCause::WrongModel.as_str().to_owned()),
                failure_cause_source: Some(FailureCauseSource::Mechanical.as_str().to_owned()),
                requested_model: Some(trial.requested_model.clone()),
            }),
        ))
    } else {
        decision
    })
}

/// The `run` entry of a grade's outcome detail: the linked run, or null.
#[must_use]
pub fn run_detail(trial: &ExperimentTrial) -> Json {
    match trial.subagent_run_id {
        Some(run) => json!({
            "subagent_run_id": run.to_string(),
            "attempt_id": trial.attempt_id.map(|id| id.to_string()),
            "model": trial.actual_model,
        }),
        None => Json::Null,
    }
}

/// The refusal of a checkout with no default branch.
#[must_use]
pub fn no_default_branch(checkout: &str) -> WgError {
    WgError::validation(format!(
        "checkout {checkout} has no default branch; set contract.base_ref on the task"
    ))
    .detail("path", checkout)
}

/// An exit code as Python reports it: Windows codes are unsigned.
#[must_use]
pub fn python_exit(code: i32) -> i64 {
    if cfg!(windows) && code < 0 {
        i64::from(code as u32)
    } else {
        i64::from(code)
    }
}

/// Seconds as the milliseconds a process timeout takes.
#[must_use]
pub fn timeout_ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1000.0) as u64
    } else {
        0
    }
}

/// A subagent run as grading needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub id: Id,
    pub attempt_id: Option<Id>,
    pub model: Option<String>,
    pub transcript: Option<String>,
}

impl RunRow {
    /// The run row as grading needs it.
    #[must_use]
    pub fn of(run: &wg_model::entity::SubagentRun) -> Self {
        Self {
            id: run.id,
            attempt_id: run.attempt_id,
            model: run.model.clone(),
            transcript: run.transcript_path.clone(),
        }
    }
}

/// The spec of a task row (`_task_spec` over its tags and acceptance criteria).
///
/// # Errors
///
/// Returns the `ValidationFailed` of a task that is neither a replay task nor a complete leaf.
pub fn spec_of(task: &wg_model::entity::Task, tags: &[String]) -> Result<Spec, WgError> {
    let criteria: Vec<String> = task.acceptance_criteria.iter().cloned().collect();
    task_spec(&task.key.to_string(), tags, &criteria, &task.metadata)
}

/// `_render_start`: the key facts of a started trial, then the brief.
#[must_use]
pub fn start_view(console: &Console, out: &Value) -> String {
    let trial = get(out, "trial");
    let title = format!(
        "trial {} of {} started",
        trial
            .and_then(|t| get(t, "seq"))
            .map(python)
            .unwrap_or_default(),
        trial
            .and_then(|t| get(t, "task_key"))
            .map(python)
            .unwrap_or_default()
    );
    let pairs = [
        ("worktree", s(get(out, "worktree"))),
        ("branch", s(get(out, "branch"))),
        ("base sha", s(get(out, "base_sha"))),
    ];
    let rows: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, Some(value.as_str())))
        .collect();
    format!(
        "{}{}\n",
        console.kv(&rows, Some(&title)).text,
        s(get(out, "brief"))
    )
}

fn exits(records: &Value) -> String {
    items(Some(records))
        .iter()
        .map(|record| s(get(record, "exit")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `_render_grade`: the outcome of a graded trial with its oracle and gate exits and its flags.
#[must_use]
pub fn grade_view(console: &Console, trial: &Value) -> String {
    let text = |name: &str| match get(trial, name) {
        None | Some(Value::Null) => String::new(),
        Some(other) => python(other),
    };
    let detail = get(trial, "outcome_detail");
    let field = |name: &str| detail.and_then(|d| get(d, name));
    let gates = field("gates").map(exits).unwrap_or_default();
    let flags = match field("flags") {
        Some(Value::Dict(entries)) if !entries.is_empty() => {
            wg_json::log_json(&Value::Dict(entries.clone())).unwrap_or_default()
        }
        _ => "none".to_owned(),
    };
    let cause = text("failure_cause");
    let failure = if cause.is_empty() {
        String::new()
    } else {
        format!("{cause} ({})", text("failure_cause_source"))
    };
    let title = format!("trial {} of {} graded", text("seq"), text("task_key"));
    let pairs = [
        ("trial", text("id")),
        ("outcome", text("outcome")),
        ("status", text("status")),
        (
            "oracle exits",
            field("oracle").map(exits).unwrap_or_default(),
        ),
        (
            "gate exits",
            if gates.is_empty() {
                "none".to_owned()
            } else {
                gates
            },
        ),
        ("flags", flags),
        ("failure cause", failure),
    ];
    let rows: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, Some(value.as_str())))
        .collect();
    console.kv(&rows, Some(&title)).text
}
