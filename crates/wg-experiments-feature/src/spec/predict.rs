//! The success predictor (`experiments/predictor.py`): `experiment fit` fits P(success) over the
//! graded trials of one model and stores the fit, and `experiment predict` reads a stored fit.
//!
//! The features are the size a leaf is predicted to have (log2 source lines, log2 files) and the
//! number of packages it touches. Each repository gets its own intercept, shrunk toward the shared
//! fit by an L2 penalty. A fit stores its data cutoff and the trials it used, and every
//! prediction names the fit it came from. The arithmetic is `wg_experiments::predictor`; this
//! module selects the rows, runs the bootstrap and shapes the stored fit and the prediction.

use serde_json::{Map, Value as Json, json};
use std::collections::BTreeMap;
use wg_experiments::predictor::{
    self, FEATURE_NAMES, FitRejection, MIN_ROWS, check_fit_rows, check_fit_settings,
    check_prediction_sizes, design_matrix, features_of, penalty_vector, prediction_interval,
};
use wg_experiments::stats::{
    DEFAULT_MAX_ITER, DEFAULT_TOL, Penalty, ValueError, brier, fit_logistic,
};
use wg_model::entity::{ExperimentTrial, JsonObject, PredictorFit};
use wg_model::event::experiment::PredictorFitted;
use wg_model::{
    Decision, EventDraft, EventPayload, RegistrySlice, Stamp, TaskSlice, Write, py_repr_str,
};
use wg_pyrand::Random;
use wg_types::{ErrorCode, Id, Timestamp, WgError};
use wg_wire::dto::predictor::Prediction;

use super::planning::replay_features;

/// One graded trial the predictor can learn from.
#[derive(Debug, Clone, PartialEq)]
pub struct FitRow {
    pub trial_id: Id,
    pub repository: Option<Id>,
    pub features: Vec<f64>,
    pub passed: bool,
    pub graded_at: Option<Timestamp>,
}

fn value_error(error: ValueError) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, error.message())
}

fn failed(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// `_number`: an int or float that is not a bool.
fn number(value: Option<&Json>) -> Option<f64> {
    match value {
        Some(Json::Number(n)) => n.as_f64(),
        _ => None,
    }
}

/// `check_fit_settings` as the validation error `fit` reports.
///
/// # Errors
///
/// Returns `validation_failed` for a negative penalty or sample count.
pub fn check_settings(
    repo_penalty: f64,
    feature_penalty: f64,
    samples: i64,
) -> Result<(), WgError> {
    check_fit_settings(repo_penalty, feature_penalty, samples).map_err(|e| failed(e.message()))
}

/// `Predictor._rows`: the graded trials of `model` (actual model, else requested) that carry
/// ex-ante features. `repository_of` gives the first repository of a task.
#[must_use]
pub fn fit_rows(
    model: &str,
    trials: &[ExperimentTrial],
    repository_of: impl Fn(Id) -> Option<Id>,
) -> Vec<FitRow> {
    trials
        .iter()
        .filter(|trial| trial.status == "graded" && trial.outcome.is_some())
        .filter(|trial| {
            trial
                .actual_model
                .as_deref()
                .filter(|actual| !actual.is_empty())
                .unwrap_or(&trial.requested_model)
                == model
        })
        .filter_map(|trial| {
            let replay = replay_features(trial)?;
            let src_lines = number(replay.get("src_lines"))?;
            let src_files = number(replay.get("src_files"))?;
            let packages = replay.get("packages")?.as_array()?.len();
            Some(FitRow {
                trial_id: trial.id,
                repository: repository_of(trial.task_id),
                features: features_of(src_lines, src_files, packages).to_vec(),
                passed: trial.outcome.as_deref() == Some("pass"),
                graded_at: trial.graded_at,
            })
        })
        .collect()
}

/// The rejection of a set of rows, as the validation error `fit` reports.
fn rejection(model: &str, rows: usize, passes: usize, why: FitRejection) -> WgError {
    match why {
        FitRejection::TooFewRows { rows } => failed(format!(
            "the predictor for {model} needs at least {MIN_ROWS} graded trials with src_lines, \
             src_files and packages; has {rows}"
        ))
        .with_detail("model", json!(model))
        .with_detail("rows", json!(rows)),
        FitRejection::OnlyPasses | FitRejection::OnlyFails => failed(format!(
            "the predictor for {model} needs both pass and fail outcomes; has only {}",
            if passes > 0 { "passes" } else { "fails" }
        ))
        .with_detail("model", json!(model))
        .with_detail("rows", json!(rows))
        .with_detail("passes", json!(passes)),
    }
}

/// A float as a JSON number.
fn number_json(value: f64) -> Json {
    serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number)
}

/// The bootstrap refits of a fit: `samples` resamples of the rows, each refitted, keeping those
/// that converged. The resampling is Python's `random.Random(seed).choices`.
fn bootstrap(
    matrix: &[Vec<f64>],
    outcomes: &[i64],
    penalty: &Penalty,
    samples: i64,
    seed: i128,
) -> Result<Vec<Vec<f64>>, WgError> {
    let indices: Vec<usize> = (0..matrix.len()).collect();
    let (draws, _) = (0..samples).try_fold(
        (Vec::<Vec<f64>>::new(), Random::new(seed)),
        |(draws, rng), _| {
            let (picked, rng) = rng
                .choices(&indices, None, None, indices.len())
                .map_err(|e| WgError::new(ErrorCode::WorkgraphError, format!("{e:?}")))?;
            let xs: Vec<Vec<f64>> = picked.iter().map(|i| matrix[**i].clone()).collect();
            let ys: Vec<i64> = picked.iter().map(|i| outcomes[**i]).collect();
            let refit = fit_logistic(&xs, &ys, penalty, DEFAULT_MAX_ITER, DEFAULT_TOL)
                .map_err(value_error)?;
            let draws = if refit.converged {
                draws.into_iter().chain([refit.coef]).collect()
            } else {
                draws
            };
            Ok::<_, WgError>((draws, rng))
        },
    )?;
    Ok(draws)
}

/// The settings of a fit request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitSettings {
    pub repo_penalty: f64,
    pub feature_penalty: f64,
    pub samples: i64,
    pub seed: i128,
}

/// `Predictor.fit`: fits the predictor for `model` over `rows`, bootstraps it and records the fit
/// and its `predictor.fitted` event.
///
/// # Errors
///
/// Returns `validation_failed` when the rows are too few or all have one outcome.
pub fn fit(
    stamp: &Stamp,
    workspace_id: Id,
    model: &str,
    rows: &[FitRow],
    settings: &FitSettings,
) -> Result<Decision<PredictorFit>, WgError> {
    let passes = rows.iter().filter(|row| row.passed).count();
    check_fit_rows(rows.len(), passes).map_err(|why| rejection(model, rows.len(), passes, why))?;
    let mut repositories: Vec<Id> = rows.iter().filter_map(|row| row.repository).collect();
    repositories.sort();
    repositories.dedup();
    let inputs: Vec<(Vec<f64>, Option<Id>)> = rows
        .iter()
        .map(|row| (row.features.clone(), row.repository))
        .collect();
    let matrix = design_matrix(&inputs, &repositories);
    let outcomes: Vec<i64> = rows.iter().map(|row| i64::from(row.passed)).collect();
    // The intercept is unpenalised; features and repository intercepts shrink toward zero.
    let penalty = Penalty::PerCoefficient(penalty_vector(
        repositories.len(),
        settings.feature_penalty,
        settings.repo_penalty,
    ));
    let fitted = fit_logistic(&matrix, &outcomes, &penalty, DEFAULT_MAX_ITER, DEFAULT_TOL)
        .map_err(value_error)?;
    let draws = bootstrap(
        &matrix,
        &outcomes,
        &penalty,
        settings.samples,
        settings.seed,
    )?;
    let probs = matrix
        .iter()
        .map(|x| predictor::probability(&fitted.coef, x))
        .collect::<Result<Vec<f64>, _>>()
        .map_err(value_error)?;
    let score = brier(&probs, &outcomes).map_err(value_error)?;
    let cutoff = rows.iter().filter_map(|row| row.graded_at).max();
    let repository_names: Vec<String> = repositories.iter().map(Id::to_string).collect();
    let metrics: JsonObject = [
        ("brier".to_owned(), number_json(score)),
        ("n_pass".to_owned(), json!(passes)),
        ("n_fail".to_owned(), json!(rows.len() - passes)),
    ]
    .into_iter()
    .collect();
    let ([fit_id, event_id], _) = stamp.ids().take::<2>();
    let record = PredictorFit {
        id: fit_id,
        workspace_id,
        model: model.to_owned(),
        created_at: stamp.at,
        data_cutoff: cutoff,
        trial_ids: rows.iter().map(|row| row.trial_id.to_string()).collect(),
        feature_names: FEATURE_NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .chain(
                repository_names
                    .iter()
                    .map(|repo| format!("repository:{repo}")),
            )
            .collect(),
        repositories: repository_names.iter().cloned().collect(),
        coef: fitted.coef.iter().map(|c| number_json(*c)).collect(),
        penalty: fitted.penalty.iter().map(|p| number_json(*p)).collect(),
        n: i64::try_from(rows.len()).unwrap_or(i64::MAX),
        converged: fitted.converged,
        metrics,
        samples: draws
            .iter()
            .map(|draw| Json::Array(draw.iter().map(|c| number_json(*c)).collect()))
            .collect(),
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        fit_id,
        EventPayload::PredictorFitted(PredictorFitted {
            converged: record.converged,
            data_cutoff: record.data_cutoff.map(Timestamp::format_isoformat),
            model: model.to_owned(),
            n: record.n,
            repositories: json!(repository_names),
            samples: json!(draws.len()),
        }),
    );
    Ok(Decision::new(record.clone())
        .write(Write::insert(record))
        .event(event))
}

/// `Predictor._fit_row`: the fit a prediction uses. `fit_id` is the requested fit, `by_id` what
/// that id names, and `latest` the newest fit of the model.
///
/// # Errors
///
/// Returns `not_found` when there is no such fit, and `validation_failed` when the fit is for
/// another model.
pub fn select_fit(
    model: &str,
    workspace_id: Id,
    fit_id: Option<&str>,
    by_id: Option<PredictorFit>,
    latest: Option<PredictorFit>,
) -> Result<PredictorFit, WgError> {
    let Some(requested) = fit_id else {
        return latest.ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!(
                    "no predictor fit for model {model}; create one with \
                     `workgraph experiment fit --model {model}`"
                ),
            )
            .with_detail("model", json!(model))
        });
    };
    let row = by_id
        .filter(|row| row.workspace_id == workspace_id)
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("predictor fit {} not found", py_repr_str(requested)),
            )
            .with_detail("fit", json!(requested))
        })?;
    if row.model == model {
        Ok(row)
    } else {
        Err(failed(format!(
            "predictor fit {requested} is for model {}, not {model}",
            row.model
        ))
        .with_detail("fit", json!(requested)))
    }
}

/// The first repository of each task (`task.repository_ids[0]`; the ids are kept sorted).
#[must_use]
pub fn first_repositories(slice: &TaskSlice) -> BTreeMap<Id, Id> {
    // Collected in reverse, so the smallest repository id of a task is the one that remains.
    let keys: Vec<&(Id, Id)> = slice.task_repositories.keys().collect();
    keys.into_iter()
        .rev()
        .map(|(task_id, repository_id)| (*task_id, *repository_id))
        .collect()
}

/// `Predictor._repository`: the `(name, id)` of a repository given by name or id. An unknown
/// reference keeps its text and has no id.
#[must_use]
pub fn resolve_repository(
    registry: &RegistrySlice,
    workspace_id: Id,
    reference: Option<&str>,
) -> (Option<String>, Option<Id>) {
    match reference {
        None => (None, None),
        Some(reference) => {
            match wg_registry::resolve::repository(registry, workspace_id, reference, false) {
                Ok(repo) => (Some(repo.name.clone()), Some(repo.id)),
                Err(_) => (Some(reference.to_owned()), None),
            }
        }
    }
}

/// The numbers of a stored JSON list.
fn floats(values: impl Iterator<Item = Json>) -> Vec<f64> {
    values.filter_map(|value| value.as_f64()).collect()
}

/// The size check `predict` makes before it looks anything up.
///
/// # Errors
///
/// Returns `validation_failed` for a negative size.
pub fn check_sizes(src_lines: f64, src_files: f64) -> Result<(), WgError> {
    check_prediction_sizes(src_lines, src_files).map_err(|e| failed(e.message()))
}

/// `Predictor.predict`: the probability of success for a leaf of this size and repository, with
/// a bootstrap interval when the fit stored enough samples. `repository` is the resolved
/// `(name, id)` of the repository the request named.
///
/// # Errors
///
/// Returns `validation_failed` for a fit whose layout does not match the features.
pub fn prediction(
    fit: &PredictorFit,
    repository: &(Option<String>, Option<Id>),
    src_lines: f64,
    src_files: f64,
    packages: usize,
) -> Result<Prediction, WgError> {
    let (repo_name, repo_id) = repository;
    let known = repo_id.map(|id| id.to_string());
    let indicators = fit.repositories.iter().map(|repo| {
        if known.as_ref() == Some(repo) {
            1.0
        } else {
            0.0
        }
    });
    let x: Vec<f64> = features_of(src_lines, src_files, packages)
        .into_iter()
        .chain(indicators)
        .collect();
    let coef = floats(fit.coef.iter().cloned());
    let draws: Vec<Vec<f64>> = fit
        .samples
        .iter()
        .map(|draw| floats(draw.as_array().into_iter().flatten().cloned()))
        .collect();
    let probability = predictor::probability(&coef, &x).map_err(value_error)?;
    let interval = prediction_interval(&draws, &x).map_err(value_error)?;
    Ok(Prediction {
        probability,
        low: interval.map(|(low, _)| low),
        high: interval.map(|(_, high)| high),
        fit_id: fit.id,
        model: fit.model.clone(),
        data_cutoff: fit.data_cutoff,
        n: fit.n,
        repository: repo_name.clone(),
        repository_known: known.is_some_and(|id| fit.repositories.iter().any(|repo| *repo == id)),
    })
}

/// What `experiment predict` was asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum PredictRequest {
    /// A task whose `contract.predicted` metadata gives the features.
    Task(String),
    /// The repository and sizes given on the command line.
    Sizes {
        repository: String,
        src_lines: f64,
        src_files: f64,
        packages: usize,
    },
}

/// The command-line options of `experiment predict`, as given.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictOptions {
    pub task: Option<String>,
    pub repo: Option<String>,
    pub lines: Option<f64>,
    pub files: Option<f64>,
    pub packages: Option<String>,
}

/// The `--task` form or the `--repo, --lines, --files, --packages` form of `experiment predict`.
///
/// # Errors
///
/// Returns `validation_failed` when both forms are mixed or the second is incomplete.
pub fn predict_request(options: &PredictOptions) -> Result<PredictRequest, WgError> {
    if let Some(task) = &options.task {
        let others = options.repo.is_some()
            || options.lines.is_some()
            || options.files.is_some()
            || options.packages.is_some();
        return if others {
            Err(failed(
                "give either --task or --repo, --lines, --files and --packages, not both",
            ))
        } else {
            Ok(PredictRequest::Task(task.clone()))
        };
    }
    match (
        &options.repo,
        options.lines,
        options.files,
        &options.packages,
    ) {
        (Some(repo), Some(lines), Some(files), Some(packages)) => Ok(PredictRequest::Sizes {
            repository: repo.clone(),
            src_lines: lines,
            src_files: files,
            packages: packages
                .split(',')
                .filter(|name| !name.trim().is_empty())
                .count(),
        }),
        _ => {
            let missing: Vec<&str> = [
                ("--repo", options.repo.is_none()),
                ("--lines", options.lines.is_none()),
                ("--files", options.files.is_none()),
                ("--packages", options.packages.is_none()),
            ]
            .into_iter()
            .filter(|(_, absent)| *absent)
            .map(|(flag, _)| flag)
            .collect();
            Err(failed(format!(
                "predict needs --task or all of --repo, --lines, --files, --packages; missing {}",
                missing.join(", ")
            )))
        }
    }
}

/// `Predictor.predict_task`: the sizes a task's `metadata.contract.predicted` declares.
///
/// # Errors
///
/// Returns `validation_failed` when the task declares no prediction or lacks a field.
pub fn task_features(
    task_key: &str,
    metadata: &Map<String, Json>,
) -> Result<(f64, f64, usize), WgError> {
    let missing = |field: &str, what: &str| {
        failed(format!(
            "task {task_key} is missing contract.predicted.{field} ({what})"
        ))
        .with_detail("task", json!(task_key))
        .with_detail("field", json!(field))
    };
    let predicted = metadata
        .get("contract")
        .and_then(Json::as_object)
        .and_then(|contract| contract.get("predicted"))
        .and_then(Json::as_object)
        .ok_or_else(|| {
            failed(format!(
                "task {task_key} has no metadata contract.predicted (src_lines, src_files, packages)"
            ))
            .with_detail("task", json!(task_key))
        })?;
    let size = |field: &str| {
        number(predicted.get(field))
            .filter(|value| *value >= 0.0)
            .ok_or_else(|| missing(field, "a non-negative number"))
    };
    let src_lines = size("src_lines")?;
    let src_files = size("src_files")?;
    let packages = predicted
        .get("packages")
        .and_then(Json::as_array)
        .ok_or_else(|| missing("packages", "a list"))?
        .len();
    Ok((src_lines, src_files, packages))
}
