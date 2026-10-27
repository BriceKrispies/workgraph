//! Pure: the decisions of the trial commands `trial add`, `trial classify` and the trial read
//! (`trial show`), the `ExperimentService` methods `add_trial`, `classify` and `trial` (IF-20,
//! C079, C080, C082).
//!
//! The shell in [`crate::service::trials`] loads the experiment, its task and the next sequence
//! number, runs these functions and applies the decision. Every error text and detail is the one
//! the Python service raises, in the same order of checks.

use std::collections::BTreeMap;

use serde_json::Value as Json;
use wg_clispec::args::{get, opt, s};
use wg_experiments::trials as lifecycle;
use wg_json::Value;
use wg_model::entity::{Experiment, ExperimentTrial, Task};
use wg_model::event::{EventPayload, ExperimentTrialAdded, ExperimentTrialClassified};
use wg_model::{Decision, EventDraft, Row, Stamp, Write, py_repr_str};
use wg_render::Console;
use wg_types::{
    ExperimentStatus, FailureCause, FailureCauseSource, TrialOutcome, TrialPhase, TrialStatus,
    WgError,
};
use wg_wire::dto::trial::from_model;

/// `_choice`: the enum member whose wire string is `value`, or the Python `ValidationFailed`.
///
/// `what` is the noun the message names (`phase`, `failure cause`, ...). The allowed values are
/// listed in declaration order, as Python lists the members of its `StrEnum`.
pub fn choice<E: Copy>(
    value: &str,
    what: &str,
    all: &[E],
    text: fn(E) -> &'static str,
) -> Result<E, WgError> {
    match all.iter().copied().find(|member| text(*member) == value) {
        Some(member) => Ok(member),
        None => {
            let allowed: Vec<&str> = all.iter().map(|member| text(*member)).collect();
            Err(WgError::validation(format!(
                "{what} {} is not one of: {}",
                py_repr_str(value),
                allowed.join(", ")
            ))
            .detail("allowed", allowed))
        }
    }
}

/// The `add_trial` input after the shell has read the CLI arguments.
#[derive(Clone, Copy, Debug)]
pub struct AddInput<'a> {
    pub arm: &'a str,
    pub phase: &'a str,
    pub band: Option<i64>,
}

/// `ExperimentService.add_trial` over the loaded experiment and task. `next_seq` is one more than
/// the highest sequence number the experiment has.
///
/// # Errors
///
/// Returns the `ConflictError` of an experiment that is not running, the `ValidationFailed` of an
/// arm outside the protocol, an unknown phase or a band outside the protocol range.
pub fn add(
    experiment: &Experiment,
    task: &Task,
    next_seq: i64,
    input: AddInput<'_>,
    stamp: &Stamp,
) -> Result<Decision<Value>, WgError> {
    require_running(experiment)?;
    let arms = arms_of(experiment);
    let model = match arms.get(input.arm) {
        Some(model) => model.clone(),
        None => {
            let names: Vec<&str> = arms.keys().map(String::as_str).collect();
            return Err(WgError::validation(format!(
                "arm {} is not in the protocol of {}",
                py_repr_str(input.arm),
                experiment.slug
            ))
            .detail("arms", names));
        }
    };
    let phase = choice(input.phase, "phase", TrialPhase::ALL, TrialPhase::as_str)?;
    check_band(experiment, input.band)?;
    let ids = stamp.ids();
    let (trial_id, ids) = ids.next();
    let (event_id, _) = ids.next();
    let trial = ExperimentTrial {
        id: trial_id,
        experiment_id: experiment.id,
        task_id: task.id,
        attempt_id: None,
        subagent_run_id: None,
        arm: input.arm.to_owned(),
        requested_model: model.clone(),
        actual_model: None,
        phase: phase.as_str().to_owned(),
        seq: next_seq,
        band: input.band,
        features: serde_json::Map::from_iter([(
            "predicted".to_owned(),
            Json::Object(serde_json::Map::new()),
        )]),
        base_ref: None,
        worktree_path: None,
        status: TrialStatus::Planned.as_str().to_owned(),
        outcome: None,
        outcome_detail: serde_json::Map::new(),
        failure_cause: None,
        failure_cause_source: None,
        notes: None,
        created_at: stamp.at,
        started_at: None,
        graded_at: None,
    };
    let task_key = task.key.to_string();
    let reply = from_model(&trial, &task_key);
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(experiment.workspace_id),
        trial.id,
        EventPayload::ExperimentTrialAdded(ExperimentTrialAdded {
            arm: input.arm.to_owned(),
            band: input.band,
            experiment: experiment.slug.clone(),
            phase: phase.as_str().to_owned(),
            requested_model: model,
            seq: next_seq,
            task_key,
        }),
    );
    Ok(Decision::new(reply)
        .write(Write::insert(Row::ExperimentTrial(trial)))
        .event(event))
}

/// `ExperimentService.classify`: names the failure cause of a failed or invalid trial.
///
/// `notes` replaces the trial's notes only when it is given. `task_key` is the key of the trial's
/// task, for the reply.
///
/// # Errors
///
/// Returns the `ConflictError` of a non-running experiment, the `ValidationFailed` of an unknown
/// cause or source, and the `ConflictError` of a trial that is neither failed nor invalid.
pub fn classify(
    experiment: &Experiment,
    trial: &ExperimentTrial,
    task_key: &str,
    cause: &str,
    source: &str,
    notes: Option<&str>,
    stamp: &Stamp,
) -> Result<Decision<Value>, WgError> {
    require_running(experiment)?;
    let failure = choice(
        cause,
        "failure cause",
        FailureCause::ALL,
        FailureCause::as_str,
    )?;
    let decided_by = choice(
        source,
        "failure cause source",
        FailureCauseSource::ALL,
        FailureCauseSource::as_str,
    )?;
    let status = parse_status(&trial.status)?;
    let outcome = trial.outcome.as_deref().and_then(TrialOutcome::parse);
    lifecycle::classify(status, outcome).map_err(|rejected| {
        WgError::conflict(format!("trial {} {}", trial.id, rejected.message()))
            .detail("trial", trial.id.to_string())
            .detail("status", &trial.status)
            .detail("outcome", &trial.outcome)
    })?;
    let next = ExperimentTrial {
        failure_cause: Some(failure.as_str().to_owned()),
        failure_cause_source: Some(decided_by.as_str().to_owned()),
        notes: notes.map_or_else(|| trial.notes.clone(), |text| Some(text.to_owned())),
        ..trial.clone()
    };
    let reply = from_model(&next, task_key);
    let event = EventDraft::new(
        stamp,
        stamp.ids().next().0,
        Some(experiment.workspace_id),
        trial.id,
        EventPayload::ExperimentTrialClassified(ExperimentTrialClassified {
            experiment: experiment.slug.clone(),
            failure_cause: Some(failure.as_str().to_owned()),
            failure_cause_source: Some(decided_by.as_str().to_owned()),
        }),
    );
    Ok(Decision::new(reply)
        .write(Write::update(Row::ExperimentTrial(next)))
        .event(event))
}

/// `_require_running`: trials change only while their experiment runs.
pub(crate) fn require_running(experiment: &Experiment) -> Result<(), WgError> {
    if experiment.status == ExperimentStatus::Running.as_str() {
        return Ok(());
    }
    Err(WgError::conflict(format!(
        "experiment {} is {}; trials change only while it is running",
        experiment.slug, experiment.status
    ))
    .detail("experiment", &experiment.slug)
    .detail("status", &experiment.status))
}

/// `_arms`: the arm names of the protocol with their requested models, sorted by name.
fn arms_of(experiment: &Experiment) -> BTreeMap<String, String> {
    experiment
        .protocol
        .get("arms")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|arm| {
            let name = arm.get("name")?.as_str()?;
            let model = arm.get("model")?.as_str()?;
            Some((name.to_owned(), model.to_owned()))
        })
        .collect()
}

/// The band check of `add_trial`: a band, when given, lies in the protocol's range.
fn check_band(experiment: &Experiment, band: Option<i64>) -> Result<(), WgError> {
    let bands = experiment.protocol.get("bands");
    let low = bands.and_then(|b| b.get("min")).and_then(Json::as_i64);
    let high = bands.and_then(|b| b.get("max")).and_then(Json::as_i64);
    match (band, low, high) {
        (Some(value), Some(min), Some(max)) if !(min <= value && value <= max) => {
            Err(WgError::validation(format!(
                "band {value} is outside the protocol range {min}..{max}"
            ))
            .detail("band", value)
            .detail("min", min)
            .detail("max", max))
        }
        _ => Ok(()),
    }
}

/// A stored trial status; an unknown string is a corrupt row.
pub(crate) fn parse_status(text: &str) -> Result<TrialStatus, WgError> {
    TrialStatus::parse(text)
        .ok_or_else(|| WgError::workgraph(format!("unknown trial status {}", py_repr_str(text))))
}

/// `--band`: an integer, or none. Click types the option as `int`, so a non-integer is a usage
/// error the parser reports first; this only converts the text that remains.
///
/// # Errors
///
/// Returns a `ValidationFailed` when the text is not an integer.
pub fn band_text(text: Option<&str>) -> Result<Option<i64>, WgError> {
    match text {
        None => Ok(None),
        Some(text) => text
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| WgError::validation(format!("invalid band {text:?}"))),
    }
}

/// The human view of a trial (`_render_trial`): the title names the sequence number and the task.
#[must_use]
pub fn trial_view(console: &Console, trial: &Value) -> String {
    let text = |name: &str| opt(get(trial, name)).unwrap_or_default();
    let cause = text("failure_cause");
    let failure = if cause.is_empty() {
        String::new()
    } else {
        format!("{cause} ({})", text("failure_cause_source"))
    };
    let title = format!("trial {} of {}", s(get(trial, "seq")), text("task_key"));
    let pairs = [
        ("trial", text("id")),
        ("experiment", text("experiment_id")),
        ("task", text("task_key")),
        ("arm", text("arm")),
        ("phase", text("phase")),
        ("seq", s(get(trial, "seq"))),
        ("band", text("band")),
        ("requested model", text("requested_model")),
        ("actual model", text("actual_model")),
        ("status", text("status")),
        ("outcome", text("outcome")),
        ("failure cause", failure),
        ("attempt", text("attempt_id")),
        ("subagent run", text("subagent_run_id")),
        ("base ref", text("base_ref")),
        ("worktree", text("worktree_path")),
        ("notes", text("notes")),
    ];
    let rows: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, Some(value.as_str())))
        .collect();
    console.kv(&rows, Some(&title)).text
}
