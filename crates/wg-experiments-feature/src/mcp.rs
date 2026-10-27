//! The experiment tools (`mcp/server.py`, M17 to M22): `experiment_list`, `experiment_get`,
//! `experiment_next`, `experiment_report`, `experiment_fit` and `experiment_predict`.
//!
//! Each call runs the [`crate::service`] entry point the CLI runs, under the agent's identity
//! (`agent_context`), and the service checks the permissions its Python method checks. The
//! arguments arrive validated, with defaults filled in, by the catalog. The trial tools (M23 to
//! M25: `trial_classify`, `trial_start`, `trial_grade`) run the trial and runner services.

use serde_json::Value;
use wg_json::Value as Dto;
use wg_mcpspec::options::Agent;
use wg_mcpspec::{structured_output, text_argument};
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;
use wg_wire::dto::experiment_shapes;

use crate::service::{self, runner, trials};
use crate::spec::predict::{FitSettings, PredictRequest};

/// The feature's tool descriptor: one [`call`] that answers the experiment tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// Answers tool `name` when this module owns it; `None` leaves the call to the backend's fallback.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "experiment_list" => list(app, agent, arguments),
        "experiment_get" => get(app, agent, arguments),
        "experiment_next" => next(app, agent, arguments),
        "experiment_report" => report(app, agent, arguments),
        "experiment_fit" => fit(app, agent, arguments),
        "experiment_predict" => predict(app, agent, arguments),
        "trial_classify" => classify(app, agent, arguments),
        "trial_start" => start(app, agent, arguments),
        "trial_grade" => grade(app, agent, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// `trial_classify`: records the cause of a failed trial; the trial as `TrialOut`.
fn classify(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    trials::trial_classify(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "trial").unwrap_or_default(),
        text_argument(arguments, "cause").unwrap_or_default(),
        text_argument(arguments, "source").unwrap_or_default(),
        text_argument(arguments, "notes"),
    )
}

/// `trial_start`: clones the replay base for a planned trial; the worktree, branch, base sha and
/// the brief for the subagent.
fn start(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    runner::trial_start_run(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "trial").unwrap_or_default(),
    )
}

/// `trial_grade`: measures a started trial's clone and records the outcome. `timeout` defaults to
/// the catalog's 3600 seconds.
fn grade(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let timeout = arguments
        .get("timeout")
        .and_then(Value::as_f64)
        .unwrap_or(3600.0);
    runner::trial_grade_run(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "trial").unwrap_or_default(),
        timeout,
    )
}

/// `experiment_list`: `{"experiments": [ExperimentOut, ...]}`, oldest first.
fn list(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let items = service::experiment_list(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "status"),
    )?;
    Ok(match items {
        Dto::List(rows) => experiment_shapes::experiments(rows),
        other => other,
    })
}

/// `experiment_get`: `{"experiment": ExperimentOut, "trials": [TrialOut, ...]}`.
fn get(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::experiment_show(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "experiment").unwrap_or_default(),
    )
}

/// `experiment_next`: the pending trial, or the next one planned from the protocol, or `done`.
fn next(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::experiment_next(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "experiment").unwrap_or_default(),
    )
}

/// `experiment_report`: the per-arm summary of the experiment.
fn report(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::experiment_report(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text_argument(arguments, "experiment").unwrap_or_default(),
    )
}

/// `experiment_fit`: fits, bootstraps and stores the predictor for one model. The defaults are
/// the catalog's (`repo_penalty` 1.0, `feature_penalty` 0.01, `samples` 200, `seed` 0).
fn fit(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let settings = FitSettings {
        repo_penalty: arguments
            .get("repo_penalty")
            .and_then(Value::as_f64)
            .unwrap_or(1.0),
        feature_penalty: arguments
            .get("feature_penalty")
            .and_then(Value::as_f64)
            .unwrap_or(0.01),
        samples: arguments
            .get("samples")
            .and_then(Value::as_i64)
            .unwrap_or(200),
        seed: arguments
            .get("seed")
            .and_then(Value::as_i64)
            .map_or(0, i128::from),
    };
    let model = text_argument(arguments, "model").unwrap_or_default();
    service::experiment_fit(
        app,
        &context,
        text_argument(arguments, "workspace"),
        model,
        &settings,
    )
}

/// `experiment_predict`: `P(success)` for a model, from a task's `contract.predicted` metadata
/// (when `task` is given, ignoring the other features) or from the repository and sizes given.
fn predict(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let request = match arguments.get("task").and_then(Value::as_str) {
        Some(task) => PredictRequest::Task(task.to_owned()),
        None => sizes(arguments)?,
    };
    let model = text_argument(arguments, "model").unwrap_or_default();
    service::experiment_predict(
        app,
        &context,
        text_argument(arguments, "workspace"),
        model,
        &request,
        text_argument(arguments, "fit"),
    )
}

/// The size form of `experiment_predict`: `repository`, `src_lines`, `src_files` and `packages`
/// must all be given. The missing ones are named in the `validation_failed` error, as Python does.
fn sizes(arguments: &Value) -> Result<PredictRequest, WgError> {
    let repository = arguments.get("repository").and_then(Value::as_str);
    let src_lines = arguments.get("src_lines").and_then(Value::as_f64);
    let src_files = arguments.get("src_files").and_then(Value::as_f64);
    let packages = arguments.get("packages").and_then(Value::as_array);
    match (repository, src_lines, src_files, packages) {
        (Some(repository), Some(src_lines), Some(src_files), Some(packages)) => {
            Ok(PredictRequest::Sizes {
                repository: repository.to_owned(),
                src_lines,
                src_files,
                packages: packages.len(),
            })
        }
        _ => {
            let missing: Vec<&str> = [
                ("repository", repository.is_none()),
                ("src_lines", src_lines.is_none()),
                ("src_files", src_files.is_none()),
                ("packages", packages.is_none()),
            ]
            .into_iter()
            .filter(|(_, absent)| *absent)
            .map(|(name, _)| name)
            .collect();
            Err(WgError::validation(format!(
                "experiment_predict needs task or all of the features; missing {}",
                missing.join(", ")
            )))
        }
    }
}
