//! The trial planner (`experiments/planner.py`): `experiment next` chooses the next trial of an
//! experiment from its protocol, its trial history and its seed, and `experiment report`
//! summarises the same history per arm.
//!
//! `next` follows the protocol. The primary arm (`arms[0]`) walks the band staircase, refines
//! around its threshold and repeats some of its tasks; the control arm then runs tasks around the
//! threshold. Every choice is a pure function of the protocol, the history and the seed. The
//! statistics themselves are `wg_experiments`; this module reads the protocol and the trial rows,
//! orders the phases and shapes the result.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value as Json, json};
use wg_experiments::planner::{self, SizeFit, SizeFitOutcome, SizeTrial, Task, TrialFact};
use wg_experiments::pool::POOL_TAG;
use wg_experiments::staircase::{Lookup, ProtocolFields, Raw, StaircaseConfig, Step, next_step};
use wg_experiments::stats::ValueError;
use wg_json::Value;
use wg_model::entity::{Experiment, ExperimentTrial, JsonObject, SubagentRun, Task as TaskRow};
use wg_model::event::experiment::ExperimentTrialAdded;
use wg_model::slice::TaskSlice;
use wg_model::{Decision, EventDraft, EventPayload, Row, Stamp, Write, py_repr_str};
use wg_types::{Id, WgError};
use wg_validate::pydantic::py_repr;
use wg_wire::convention::{dict, int, optional_float, text};
use wg_wire::dto::{experiment_shapes, trial};

use super::lifecycle::RUNNING;
use super::protocol::arms;
use super::sql::TrialWithTaskKey;

/// A replay task the planner may draw from: its key, its band and the replay features it was
/// validated with.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub task_id: Id,
    pub band: i64,
    pub replay: JsonObject,
}

/// The trial `experiment next` adds.
#[derive(Debug, Clone, PartialEq)]
pub struct Addition {
    pub candidate: Candidate,
    pub arm: String,
    pub phase: String,
    pub band: i64,
    /// The band the staircase asked for, when a substitute task of another band was chosen.
    pub band_requested: Option<i64>,
}

/// What `experiment next` does.
#[derive(Debug, Clone, PartialEq)]
pub enum Planned {
    /// The trial at this index of the history is planned or running.
    Pending(usize),
    /// Nothing more to run, and why: `budget`, `pool_exhausted` or `complete`.
    Done(&'static str),
    /// A new trial to add.
    Add(Addition),
}

/// The plan and the staircase state it was made from.
#[derive(Debug, Clone, PartialEq)]
pub struct NextPlan {
    pub planned: Planned,
    pub step: Step,
}

/// A statistics `ValueError` as the generic domain error.
fn value_error(error: ValueError) -> WgError {
    WgError::workgraph(error.message())
}

fn object(value: Option<&Json>) -> Option<&JsonObject> {
    value.and_then(Json::as_object)
}

/// `_number`: an int or float that is not a bool.
fn number(value: Option<&Json>) -> Option<f64> {
    match value {
        Some(Json::Number(n)) => n.as_f64(),
        _ => None,
    }
}

/// A JSON integer that is not a bool.
fn integer(value: Option<&Json>) -> Option<i64> {
    match value {
        Some(Json::Number(n)) => n.as_i64(),
        _ => None,
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(flag) => *flag,
        Json::Number(n) => n.as_f64().is_none_or(|x| x != 0.0),
        Json::String(text) => !text.is_empty(),
        Json::Array(items) => !items.is_empty(),
        Json::Object(map) => !map.is_empty(),
    }
}

/// `_replay_features`: the replay features a trial was planned with (`add_trial` stores them
/// under `predicted`).
#[must_use]
pub fn replay_features(trial: &ExperimentTrial) -> Option<&JsonObject> {
    object(object(trial.features.get("predicted")).and_then(|predicted| predicted.get("replay")))
        .or_else(|| object(trial.features.get("replay")))
}

/// `_requested_band`: the band the staircase asked for. A substituted task keeps the requested
/// band for visit grouping.
fn requested_band(trial: &ExperimentTrial) -> Option<i64> {
    integer(object(trial.features.get("predicted")).and_then(|p| p.get("band_requested")))
        .or(trial.band)
}

/// `_primary`: the name of the first arm.
fn primary(protocol: &JsonObject) -> String {
    arms(protocol)
        .into_iter()
        .next()
        .map(|(name, _)| name)
        .unwrap_or_default()
}

/// `_count`: `protocol[section]["trials"]` when it is an integer, else 0.
fn count(protocol: &JsonObject, section: &str) -> i64 {
    integer(object(protocol.get(section)).and_then(|map| map.get("trials"))).unwrap_or(0)
}

/// `int(protocol.get("seed", 0))`.
fn seed(protocol: &JsonObject) -> i128 {
    match protocol.get("seed") {
        Some(Json::Number(n)) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
            .unwrap_or(0),
        _ => 0,
    }
}

/// A protocol value as the staircase reads it.
fn raw(value: &Json) -> Raw {
    match value {
        Json::Bool(flag) => Raw::Bool(*flag),
        Json::Number(n) if n.is_i64() => Raw::Int(n.as_i64().unwrap_or(0)),
        other => Raw::Other(py_repr(other)),
    }
}

fn lookup(section: Option<&JsonObject>, key: &str) -> Lookup {
    section
        .and_then(|map| map.get(key))
        .map_or(Lookup::Absent, |value| Lookup::Found(raw(value)))
}

/// The keys of `StaircaseConfig.from_protocol`.
fn protocol_fields(protocol: &JsonObject) -> ProtocolFields {
    let bands = object(protocol.get("bands"));
    let staircase = object(protocol.get("staircase"));
    let refine = object(protocol.get("refine"));
    ProtocolFields {
        has_bands: protocol.contains_key("bands"),
        has_staircase: protocol.contains_key("staircase"),
        min: lookup(bands, "min"),
        max: lookup(bands, "max"),
        per_visit: lookup(staircase, "per_visit"),
        up: lookup(staircase, "up"),
        down: lookup(staircase, "down"),
        max_reversals: lookup(staircase, "max_reversals"),
        max_trials: lookup(staircase, "max_trials"),
        refine_trials: refine.and_then(|map| map.get("trials")).map(raw),
        refine_width: refine.and_then(|map| map.get("width")).map(raw),
    }
}

/// `_step`: the staircase state, from the primary arm's graded trials only.
fn step_of(protocol: &JsonObject, trials: &[TrialWithTaskKey]) -> Result<Step, WgError> {
    let config = StaircaseConfig::from_protocol(&protocol_fields(protocol)).map_err(value_error)?;
    let facts: Vec<TrialFact> = trials
        .iter()
        .map(|row| TrialFact {
            arm: row.trial.arm.clone(),
            graded: row.trial.status == "graded",
            phase: row.trial.phase.clone(),
            band: requested_band(&row.trial),
            passed: row.trial.outcome.as_deref() == Some("pass"),
        })
        .collect();
    let (staircase, refine) = planner::staircase_histories(&primary(protocol), &facts);
    next_step(&config, &staircase, &refine).map_err(value_error)
}

/// `_graded_tasks`: primary-arm graded tasks, one per key, optionally limited to some phases,
/// sorted by key.
fn graded_tasks(
    valid: &[&TrialWithTaskKey],
    primary: &str,
    phases: Option<&[&str]>,
) -> Vec<Candidate> {
    let found = valid
        .iter()
        .filter(|row| {
            row.trial.arm == primary && row.trial.status == "graded" && row.trial.band.is_some()
        })
        .filter(|row| phases.is_none_or(|phases| phases.contains(&row.trial.phase.as_str())))
        .filter_map(|row| {
            Some(Candidate {
                key: row.task_key.clone(),
                task_id: row.trial.task_id,
                band: row.trial.band?,
                replay: replay_features(&row.trial)?.clone(),
            })
        })
        .fold(Vec::<Candidate>::new(), |found, candidate| {
            if found.iter().any(|have| have.key == candidate.key) {
                found
            } else {
                found.into_iter().chain([candidate]).collect()
            }
        });
    let mut sorted = found;
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    sorted
}

fn as_tasks(candidates: &[Candidate]) -> Vec<Task> {
    candidates
        .iter()
        .map(|candidate| Task {
            key: candidate.key.clone(),
            band: candidate.band,
        })
        .collect()
}

/// `_pick`: the same seed, history position and candidates give the same task.
fn pick(seed: i128, next_seq: i64, candidates: &[Candidate]) -> Option<&Candidate> {
    let tasks = as_tasks(candidates);
    let chosen = planner::pick(seed, i128::from(next_seq), &tasks)?;
    candidates
        .iter()
        .find(|candidate| candidate.key == chosen.key)
}

fn addition(
    candidate: &Candidate,
    arm: &str,
    phase: &str,
    band: i64,
    band_requested: Option<i64>,
) -> Planned {
    Planned::Add(Addition {
        candidate: candidate.clone(),
        arm: arm.to_owned(),
        phase: phase.to_owned(),
        band,
        band_requested,
    })
}

/// The plan for the trials past the staircase: the repeat trials, then the control arm.
fn plan_after_staircase(
    protocol: &JsonObject,
    step: &Step,
    valid: &[&TrialWithTaskKey],
    primary: &str,
    seed: i128,
    next_seq: i64,
) -> Planned {
    let repeats: Vec<&&TrialWithTaskKey> = valid
        .iter()
        .filter(|row| row.trial.phase == "repeat")
        .collect();
    let repeated: BTreeSet<&str> = repeats.iter().map(|row| row.task_key.as_str()).collect();
    let repeat_pick = if i64::try_from(repeats.len()).unwrap_or(i64::MAX)
        < count(protocol, "repeat")
    {
        let tasks: Vec<Candidate> = graded_tasks(valid, primary, Some(&["staircase", "refine"]))
            .into_iter()
            .filter(|task| !repeated.contains(task.key.as_str()))
            .collect();
        pick(seed, next_seq, &tasks).map(|task| addition(task, primary, "repeat", task.band, None))
    } else {
        None
    };
    if let Some(planned) = repeat_pick {
        return planned;
    }
    let control_arm = object(protocol.get("control"))
        .and_then(|control| control.get("arm"))
        .and_then(Json::as_str);
    let control_pick = control_arm.and_then(|control_arm| {
        let ran: Vec<&&TrialWithTaskKey> = valid
            .iter()
            .filter(|row| row.trial.arm == control_arm)
            .collect();
        let remaining = count(protocol, "control") - i64::try_from(ran.len()).unwrap_or(i64::MAX);
        if remaining <= 0 {
            return None;
        }
        let ran_keys: BTreeSet<&str> = ran.iter().map(|row| row.task_key.as_str()).collect();
        let available: Vec<Candidate> = graded_tasks(valid, primary, None)
            .into_iter()
            .filter(|task| !ran_keys.contains(task.key.as_str()))
            .collect();
        let near: Vec<Candidate> = step
            .threshold
            .map(f64::round_ties_even)
            .map(|centre| centre as i64)
            .map(|centre| {
                available
                    .iter()
                    .filter(|task| task.band == centre || task.band == centre + 1)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let candidates = if i64::try_from(near.len()).unwrap_or(0) >= remaining {
            near
        } else {
            available
        };
        pick(seed, next_seq, &candidates)
            .map(|task| addition(task, control_arm, "control", task.band, None))
    });
    control_pick.unwrap_or(Planned::Done("complete"))
}

/// `TrialPlanner.next` without the write: the pending trial, the trial to add, or why the
/// experiment is done. `load_pool` runs only when the staircase draws from the pool.
///
/// # Errors
///
/// Returns the error of the staircase (a protocol it cannot read) or of `load_pool`.
pub fn plan_next(
    protocol: &JsonObject,
    trials: &[TrialWithTaskKey],
    load_pool: impl FnOnce() -> Result<Vec<Candidate>, WgError>,
) -> Result<NextPlan, WgError> {
    let step = step_of(protocol, trials)?;
    let primary = primary(protocol);
    let seed = seed(protocol);
    let pending = trials
        .iter()
        .position(|row| row.trial.status == "planned" || row.trial.status == "running");
    let valid: Vec<&TrialWithTaskKey> = trials
        .iter()
        .filter(|row| row.trial.status != "invalid")
        .collect();
    let budget =
        integer(object(protocol.get("budget")).and_then(|map| map.get("trials"))).unwrap_or(0);
    let next_seq = trials.iter().map(|row| row.trial.seq).max().unwrap_or(0) + 1;
    let planned = if let Some(index) = pending {
        Planned::Pending(index)
    } else if i64::try_from(valid.len()).unwrap_or(i64::MAX) >= budget {
        Planned::Done("budget")
    } else if let Some(band) = step.band {
        let pool = load_pool()?;
        let used: BTreeSet<String> = valid
            .iter()
            .filter(|row| row.trial.arm == primary)
            .map(|row| row.task_key.clone())
            .collect();
        let tasks = as_tasks(&pool);
        match planner::nearest_unused(&tasks, &used, band) {
            None => Planned::Done("pool_exhausted"),
            Some((chosen, unused)) => {
                let candidates: Vec<Candidate> = pool
                    .iter()
                    .filter(|candidate| unused.iter().any(|task| task.key == candidate.key))
                    .cloned()
                    .collect();
                pick(seed, next_seq, &candidates).map_or(Planned::Done("pool_exhausted"), |task| {
                    addition(
                        task,
                        &primary,
                        step.phase.as_str(),
                        chosen,
                        (chosen != band).then_some(band),
                    )
                })
            }
        }
    } else {
        plan_after_staircase(protocol, &step, &valid, &primary, seed, next_seq)
    };
    Ok(NextPlan { planned, step })
}

/// `_pool`: the replay tasks the staircase may draw from, sorted by key. A task counts when it is
/// tagged, validated when the protocol asks for it, in the pool's repository when it names one,
/// and has an integer band.
///
/// # Errors
///
/// Returns `not_found` when the pool's repository does not exist, and `validation_failed` when the
/// pool's tag or repository is not text.
pub fn pool_tasks(
    slice: &TaskSlice,
    workspace_id: Id,
    protocol: &JsonObject,
) -> Result<Vec<Candidate>, WgError> {
    let spec = object(protocol.get("pool"));
    let setting = |name: &str| spec.and_then(|map| map.get(name));
    let tag = match setting("tag") {
        None => POOL_TAG,
        Some(Json::String(tag)) => tag.as_str(),
        Some(_) => return Err(bad_pool("tag")),
    };
    let repository = match setting("repository") {
        None | Some(Json::Null) => None,
        Some(Json::String(reference)) => Some(
            wg_registry::resolve::repository(&slice.registry, workspace_id, reference, true)?.id,
        ),
        Some(_) => return Err(bad_pool("repository")),
    };
    let require_validated = setting("require_validated").is_none_or(truthy);
    let found = slice
        .tasks
        .values()
        .filter(|task| task.workspace_id == workspace_id && task.archived_at.is_none())
        .filter(|task| slice.tags_of(task.id).any(|have| have == tag))
        .filter(|task| {
            repository.is_none_or(|repository| {
                slice.task_repositories.contains_key(&(task.id, repository))
            })
        })
        .filter_map(|task| candidate_of(task, require_validated));
    let mut sorted: Vec<Candidate> = found.collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(sorted)
}

fn bad_pool(name: &str) -> WgError {
    WgError::validation(format!("the pool {name} of the protocol must be text"))
}

/// A task as a pool candidate: its `metadata.replay` must be non-empty (and validated, when
/// required) and its features must carry an integer band.
fn candidate_of(task: &TaskRow, require_validated: bool) -> Option<Candidate> {
    let replay = object(task.metadata.get("replay")).filter(|replay| !replay.is_empty())?;
    if require_validated && replay.get("validated") != Some(&Json::Bool(true)) {
        return None;
    }
    let features = object(replay.get("features")).cloned().unwrap_or_default();
    let band = integer(features.get("band"))?;
    Some(Candidate {
        key: task.key.to_string(),
        task_id: task.id,
        band,
        replay: features,
    })
}

/// `ExperimentService.add_trial`: a planned trial of a task in a running experiment. `max_seq` is
/// the highest sequence number the experiment has.
///
/// # Errors
///
/// Returns `conflict` unless the experiment is running, and `validation_failed` for an arm the
/// protocol does not name or a band outside its range.
pub fn planned_trial(
    stamp: &Stamp,
    experiment: &Experiment,
    added: &Addition,
    max_seq: Option<i64>,
) -> Result<Decision<TrialWithTaskKey>, WgError> {
    if experiment.status != RUNNING {
        return Err(WgError::conflict(format!(
            "experiment {} is {}; trials change only while it is running",
            experiment.slug, experiment.status
        ))
        .detail("experiment", &experiment.slug)
        .detail("status", &experiment.status));
    }
    let models = arms(&experiment.protocol);
    let Some((_, model)) = models.iter().find(|(name, _)| *name == added.arm) else {
        let mut names: Vec<&str> = models.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        return Err(WgError::validation(format!(
            "arm {} is not in the protocol of {}",
            py_repr_str(&added.arm),
            experiment.slug
        ))
        .detail("arms", names));
    };
    let bands = object(experiment.protocol.get("bands"));
    let low = integer(bands.and_then(|map| map.get("min"))).unwrap_or(0);
    let high = integer(bands.and_then(|map| map.get("max"))).unwrap_or(0);
    if !(low..=high).contains(&added.band) {
        return Err(WgError::validation(format!(
            "band {} is outside the protocol range {low}..{high}",
            added.band
        ))
        .detail("band", added.band)
        .detail("min", low)
        .detail("max", high));
    }
    let seq = max_seq.unwrap_or(0) + 1;
    let ([trial_id, event_id], _) = stamp.ids().take::<2>();
    let predicted: Map<String, Json> = [(
        "replay".to_owned(),
        Json::Object(added.candidate.replay.clone()),
    )]
    .into_iter()
    .chain(
        added
            .band_requested
            .map(|requested| ("band_requested".to_owned(), json!(requested))),
    )
    .collect();
    let row = ExperimentTrial {
        id: trial_id,
        experiment_id: experiment.id,
        task_id: added.candidate.task_id,
        attempt_id: None,
        subagent_run_id: None,
        arm: added.arm.clone(),
        requested_model: model.clone(),
        actual_model: None,
        phase: added.phase.clone(),
        seq,
        band: Some(added.band),
        features: [("predicted".to_owned(), Json::Object(predicted))]
            .into_iter()
            .collect(),
        base_ref: None,
        worktree_path: None,
        status: "planned".to_owned(),
        outcome: None,
        outcome_detail: Map::new(),
        failure_cause: None,
        failure_cause_source: None,
        notes: None,
        created_at: stamp.at,
        started_at: None,
        graded_at: None,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(experiment.workspace_id),
        trial_id,
        EventPayload::ExperimentTrialAdded(ExperimentTrialAdded {
            arm: added.arm.clone(),
            band: Some(added.band),
            experiment: experiment.slug.clone(),
            phase: added.phase.clone(),
            requested_model: model.clone(),
            seq,
            task_key: added.candidate.key.clone(),
        }),
    );
    Ok(Decision::new(TrialWithTaskKey {
        trial: row.clone(),
        task_key: added.candidate.key.clone(),
    })
    .write(Write::insert(Row::ExperimentTrial(row)))
    .event(event))
}

// ---------------------------------------------------------------------- shaping

/// `_step_dict`: the staircase state.
#[must_use]
pub fn step_value(step: &Step) -> Value {
    experiment_shapes::step(
        step.phase.as_str(),
        step.band,
        i64::try_from(step.reversals).unwrap_or(i64::MAX),
        step.threshold,
        &step.visits,
    )
}

/// `experiment next`'s result for a plan, once any trial it adds exists.
#[must_use]
pub fn next_value(
    plan: &NextPlan,
    trials: &[TrialWithTaskKey],
    added: Option<&TrialWithTaskKey>,
) -> Value {
    let shown = |row: &TrialWithTaskKey| trial::from_model(&row.trial, &row.task_key);
    let step = step_value(&plan.step);
    match (&plan.planned, added) {
        (Planned::Pending(index), _) => {
            let row = trials.get(*index);
            experiment_shapes::next_plan(
                false,
                row.map(|row| row.trial.phase.as_str()),
                row.map(shown),
                "pending",
                step,
            )
        }
        (Planned::Done(reason), _) => experiment_shapes::next_plan(true, None, None, reason, step),
        (Planned::Add(addition), row) => experiment_shapes::next_plan(
            false,
            Some(addition.phase.as_str()),
            row.map(shown),
            "planned",
            step,
        ),
    }
}

/// The usage and telemetry of a subagent run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunUsage {
    pub usage: JsonObject,
    pub telemetry: JsonObject,
}

impl RunUsage {
    #[must_use]
    pub fn of(run: &SubagentRun) -> Self {
        RunUsage {
            usage: run.usage.clone().unwrap_or_default(),
            telemetry: run.telemetry.clone().unwrap_or_default(),
        }
    }
}

/// `_summary`: the median and mean as a dict.
fn summary_value(values: &[f64]) -> Value {
    let summary = planner::summary(values);
    dict(vec![
        ("median", optional_float(summary.median)),
        ("mean", optional_float(summary.mean)),
    ])
}

/// The numbers of `key` across the runs' dicts.
fn numbers(runs: &[&RunUsage], pick_dict: fn(&RunUsage) -> &JsonObject, key: &str) -> Vec<f64> {
    runs.iter()
        .filter_map(|run| number(pick_dict(run).get(key)))
        .collect()
}

/// `TrialPlanner._usage`: token and turn usage of each outcome's subagent runs; trials without a
/// run are counted apart.
fn usage_value(graded: &[&TrialWithTaskKey], runs: &BTreeMap<Id, RunUsage>) -> Value {
    let entry = |outcome: &str| {
        let wanted: Vec<&&TrialWithTaskKey> = graded
            .iter()
            .filter(|row| row.trial.outcome.as_deref() == Some(outcome))
            .collect();
        let found: Vec<&RunUsage> = wanted
            .iter()
            .filter_map(|row| row.trial.subagent_run_id.and_then(|id| runs.get(&id)))
            .collect();
        let missing = wanted.len() - found.len();
        dict(vec![
            ("n", int(i64::try_from(found.len()).unwrap_or(i64::MAX))),
            (
                "missing_usage",
                int(i64::try_from(missing).unwrap_or(i64::MAX)),
            ),
            (
                "output_tokens",
                summary_value(&numbers(&found, |run| &run.usage, "output_tokens")),
            ),
            (
                "cache_read_input_tokens",
                summary_value(&numbers(
                    &found,
                    |run| &run.usage,
                    "cache_read_input_tokens",
                )),
            ),
            (
                "turns",
                summary_value(&numbers(&found, |run| &run.telemetry, "turns")),
            ),
            (
                "tool_errors",
                summary_value(&numbers(&found, |run| &run.telemetry, "tool_errors")),
            ),
        ])
    };
    dict(vec![("pass", entry("pass")), ("fail", entry("fail"))])
}

fn pair(lines: Option<(f64, f64)>) -> Value {
    lines.map_or(Value::Null, |(low, high)| {
        Value::List(vec![Value::Float(low), Value::Float(high)])
    })
}

fn fit_value(fit: &SizeFit) -> Value {
    dict(vec![
        ("intercept", Value::Float(fit.intercept)),
        ("slope", Value::Float(fit.slope)),
        ("converged", Value::Bool(fit.converged)),
        ("n", int(i64::try_from(fit.n).unwrap_or(i64::MAX))),
        ("x50", optional_float(fit.x50)),
        ("x80", optional_float(fit.x80)),
        ("lines50", optional_float(fit.lines50)),
        ("lines80", optional_float(fit.lines80)),
        ("ci50", pair(fit.ci50)),
        ("ci80", pair(fit.ci80)),
    ])
}

fn to_counts(pairs: Vec<(String, usize)>) -> Vec<(String, i64)> {
    pairs
        .into_iter()
        .map(|(key, n)| (key, i64::try_from(n).unwrap_or(i64::MAX)))
        .collect()
}

/// `TrialPlanner._arm_report`: one arm's measurements.
fn arm_value(
    name: &str,
    trials: &[TrialWithTaskKey],
    runs: &BTreeMap<Id, RunUsage>,
    seed: i128,
) -> Result<Value, WgError> {
    let arm_trials: Vec<&TrialWithTaskKey> =
        trials.iter().filter(|row| row.trial.arm == name).collect();
    let graded: Vec<&TrialWithTaskKey> = arm_trials
        .iter()
        .copied()
        .filter(|row| row.trial.status == "graded" && row.trial.outcome.is_some())
        .collect();
    let size_trials: Vec<SizeTrial> = graded
        .iter()
        .map(|row| SizeTrial {
            task_id: row.trial.task_id.to_string(),
            passed: row.trial.outcome.as_deref() == Some("pass"),
            src_lines: replay_features(&row.trial)
                .and_then(|replay| number(replay.get("src_lines"))),
        })
        .collect();
    let (fit, fit_reason) = match planner::size_fit(&size_trials, seed).map_err(value_error)? {
        SizeFitOutcome::Fitted(fit) => (fit_value(&fit), None),
        SizeFitOutcome::Rejected(reason) => (Value::Null, Some(reason)),
    };
    let band_input: Vec<(Option<i64>, bool)> = graded
        .iter()
        .map(|row| (row.trial.band, row.trial.outcome.as_deref() == Some("pass")))
        .collect();
    let bands = planner::band_rows(&band_input).map_err(value_error)?;
    let causes: Vec<&str> = arm_trials
        .iter()
        .filter(|row| {
            row.trial.status == "invalid"
                || (row.trial.status == "graded" && row.trial.outcome.as_deref() == Some("fail"))
        })
        .map(|row| {
            row.trial
                .failure_cause
                .as_deref()
                .filter(|cause| !cause.is_empty())
                .unwrap_or("unclassified")
        })
        .collect();
    let statuses: Vec<&str> = arm_trials
        .iter()
        .map(|row| row.trial.status.as_str())
        .collect();
    let phases: Vec<&str> = arm_trials
        .iter()
        .map(|row| row.trial.phase.as_str())
        .collect();
    let invalid = arm_trials
        .iter()
        .filter(|row| row.trial.status == "invalid")
        .map(|row| {
            dict(vec![
                ("seq", int(row.trial.seq)),
                ("task_key", text(&row.task_key)),
                (
                    "failure_cause",
                    row.trial.failure_cause.as_deref().map_or(Value::Null, text),
                ),
                (
                    "notes",
                    row.trial.notes.as_deref().map_or(Value::Null, text),
                ),
            ])
        })
        .collect();
    Ok(experiment_shapes::arm(&experiment_shapes::ArmReport {
        arm: name.to_owned(),
        trials: experiment_shapes::trial_counts(
            i64::try_from(arm_trials.len()).unwrap_or(i64::MAX),
            &to_counts(planner::sorted_counts(&statuses)),
            &to_counts(planner::sorted_counts(&phases)),
        ),
        bands: Value::List(
            bands
                .iter()
                .map(|row| {
                    experiment_shapes::band_row(
                        row.band,
                        i64::try_from(row.n).unwrap_or(i64::MAX),
                        i64::try_from(row.passes).unwrap_or(i64::MAX),
                        row.rate,
                        row.wilson,
                    )
                })
                .collect(),
        ),
        fit,
        fit_reason,
        usage: usage_value(&graded, runs),
        failure_causes: to_counts(planner::sorted_counts(&causes)),
        invalid: Value::List(invalid),
    }))
}

/// The ids of the subagent runs the graded trials of an experiment name.
#[must_use]
pub fn run_ids(trials: &[TrialWithTaskKey]) -> BTreeSet<Id> {
    trials
        .iter()
        .filter(|row| row.trial.status == "graded" && row.trial.outcome.is_some())
        .filter_map(|row| row.trial.subagent_run_id)
        .collect()
}

/// `TrialPlanner.report`: per-arm measurements of an experiment's trials.
///
/// # Errors
///
/// Returns the error of the staircase or of the statistics.
pub fn report_value(
    experiment: &Experiment,
    trials: &[TrialWithTaskKey],
    runs: &BTreeMap<Id, RunUsage>,
) -> Result<Value, WgError> {
    let protocol = &experiment.protocol;
    let seed = seed(protocol);
    let step = step_of(protocol, trials)?;
    let arm_values = arms(protocol)
        .iter()
        .map(|(name, _)| arm_value(name, trials, runs, seed))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(experiment_shapes::report(
        &experiment.slug,
        &experiment.status,
        protocol,
        step_value(&step),
        arm_values,
    ))
}
