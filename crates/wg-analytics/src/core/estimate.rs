//! The estimate of `analytics/estimation.py`: the comparable accepted tasks of a target (or of a
//! title), scored by category, text similarity, repository overlap and shared modules, and the
//! distributions and agent recommendations of the best of them. With fewer than [`MIN_SAMPLE`]
//! comparable tasks the result is `insufficient_data` and no number is fabricated.
//!
//! The text similarity is an input. The caller runs the similar-task search of the target (or the
//! lexical search of the title) and passes the scores, because that search reads the search index,
//! not the warehouse. Everything else is a function of the source rows and the facts.
//!
//! Python's percentiles interpolate as `lo + (hi - lo) * frac`, which is not the DuckDB formula of
//! [`quantile_cont`](super::stats::quantile_cont), so [`percentile`] keeps Python's arithmetic. The
//! agent recommendations come from a `GROUP BY`, whose row order DuckDB leaves open; here the groups
//! are ordered by provider and model, and the sort keeps that order for ties.

use std::collections::BTreeMap;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_pynum::{group_sorted, sorted_distinct, stable_sort_by};
use wg_telemetry::timing::round3;

use super::stats::quantile_cont;
use crate::Facts;
use crate::source::{AnalyticsInput, AttemptRow, TaskRow, VerificationRun};
use crate::tasks::TaskFact;

/// The fewest comparable tasks an estimate is given for (`MIN_SAMPLE`).
pub const MIN_SAMPLE: usize = 3;
/// The comparable tasks an estimate draws on (`TOP_K`).
pub const TOP_K: usize = 12;
/// The lowest score a candidate needs (`MIN_SCORE`).
pub const MIN_SCORE: f64 = 0.15;

/// The uncertainty note of an `ok` estimate.
const UNCERTAINTY: &str =
    "Ranges are the 25th-75th/90th percentiles of comparable tasks; small samples are volatile.";

/// The selection-bias caveat of the agent recommendations.
const SELECTION_CAVEAT: &str = "Agent recommendations reflect which agents happened to be used on similar tasks (selection bias); they are evidence, not a controlled comparison.";

/// The request the shell resolved: the target task (excluded from the candidates) and the category,
/// repositories and text scores the candidates are compared with. `method` is the search method
/// that produced the scores (`semantic` or `lexical`).
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub target: Option<&'a str>,
    pub category: Option<&'a str>,
    pub repositories: &'a [String],
    pub method: &'a str,
    pub text_scores: &'a [(String, f64)],
}

/// A candidate that reached the score floor.
#[derive(Debug, Clone, PartialEq)]
struct Scored {
    score: f64,
    task_id: String,
    key: Option<String>,
    title: Option<String>,
    reasons: Vec<String>,
}

/// One agent group of the comparable tasks: a provider and model, its attempts and their durations.
#[derive(Debug, Clone, PartialEq)]
struct Agent {
    provider: Option<String>,
    model: String,
    attempts: usize,
    succeeded: usize,
    success_rate: Option<f64>,
    median_duration_ms: Option<f64>,
}

/// `estimate`: the estimate document of the request over the warehouse of `workspace`.
#[must_use]
pub fn estimate(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    request: &Request<'_>,
) -> Value {
    let accepted: Vec<(&str, &TaskRow)> = input
        .tasks
        .iter()
        .filter(|task| task.workspace_id.as_deref() == Some(workspace))
        .filter(|task| task.accepted_at.is_some())
        .filter_map(|task| task.id.as_deref().map(|id| (id, task)))
        .filter(|(id, _)| request.target != Some(*id))
        .collect();
    let repos_of: BTreeMap<String, Vec<String>> = group_sorted(
        input
            .task_repositories
            .iter()
            .filter_map(|link| Some((link.task_id.clone()?, link.repository_id.clone()?))),
    )
    .into_iter()
    .collect();
    let modules_of: BTreeMap<String, Vec<Option<String>>> = group_sorted(
        facts
            .task_files
            .iter()
            .filter_map(|file| Some((file.task_id.clone()?, file.module.clone()))),
    )
    .into_iter()
    .collect();
    let text_of = |id: &str| -> f64 {
        request
            .text_scores
            .iter()
            .filter(|(task, _)| task == id)
            .map(|(_, score)| *score)
            .next_back()
            .unwrap_or(0.0)
    };
    let my_repos = sorted_distinct(request.repositories.iter().cloned());
    let my_modules = request
        .target
        .and_then(|target| modules_of.get(target))
        .map(|modules| sorted_distinct(modules.iter().cloned()))
        .unwrap_or_default();
    let scored = stable_sort_by(
        accepted
            .iter()
            .filter_map(|(id, other)| {
                let text = text_of(id);
                let category_part = request
                    .category
                    .filter(|wanted| !wanted.is_empty())
                    .filter(|wanted| other.category.as_deref() == Some(*wanted))
                    .map(|wanted| (0.35, format!("same category ({wanted})")));
                let text_part = (text != 0.0).then(|| {
                    (
                        0.4 * text.min(1.0),
                        format!("{} text similarity {text:.2}", request.method),
                    )
                });
                let theirs_repos = repos_of
                    .get(*id)
                    .map(|repos| sorted_distinct(repos.iter().cloned()))
                    .unwrap_or_default();
                let repo_part = (!my_repos.is_empty() && !theirs_repos.is_empty())
                    .then(|| overlap_of(&my_repos, &theirs_repos))
                    .filter(|overlap| *overlap != 0.0)
                    .map(|overlap| (0.2 * overlap, format!("repository overlap {overlap:.2}")));
                let module_part = (!my_modules.is_empty())
                    .then(|| {
                        let theirs = modules_of
                            .get(*id)
                            .map(|modules| sorted_distinct(modules.iter().cloned()))
                            .unwrap_or_default();
                        let shared = theirs
                            .iter()
                            .filter(|module| my_modules.binary_search(module).is_ok())
                            .count();
                        (shared > 0).then(|| {
                            let union = theirs.len() + my_modules.len() - shared;
                            (
                                0.05 * shared as f64 / union as f64,
                                "touches the same modules".to_owned(),
                            )
                        })
                    })
                    .flatten();
                let kept: Vec<(f64, String)> = [category_part, text_part, repo_part, module_part]
                    .into_iter()
                    .flatten()
                    .collect();
                let score = kept.iter().fold(0.0, |sum, (part, _)| sum + part);
                (score >= MIN_SCORE).then(|| Scored {
                    score,
                    task_id: (*id).to_owned(),
                    key: other.key.clone(),
                    title: other.title.clone(),
                    reasons: kept.into_iter().map(|(_, reason)| reason).collect(),
                })
            })
            .collect::<Vec<Scored>>(),
        // Python sorts the tuples in reverse: score first, then the task id.
        |a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.task_id.cmp(&a.task_id))
        },
    );
    let chosen: Vec<Scored> = scored.into_iter().take(TOP_K).collect();
    let basis: Vec<Value> = chosen.iter().map(basis_row).collect();
    if chosen.len() < MIN_SAMPLE {
        return json!({
            "status": "insufficient_data",
            "message": format!(
                "only {} comparable accepted task(s) found; at least {MIN_SAMPLE} are required",
                chosen.len()
            ),
            "comparable_tasks": basis,
            "min_sample": MIN_SAMPLE,
        });
    }
    let ids: Vec<&str> = chosen.iter().map(|task| task.task_id.as_str()).collect();
    let fact_of: BTreeMap<&str, &TaskFact> = facts
        .task_facts
        .iter()
        .filter_map(|fact| Some((fact.task.id.as_deref()?, fact)))
        .collect();
    let chosen_facts: Vec<&TaskFact> = ids
        .iter()
        .filter_map(|id| fact_of.get(id).copied())
        .collect();
    let with_commits: Vec<&TaskFact> = chosen_facts
        .iter()
        .copied()
        .filter(|fact| fact.commits != 0)
        .collect();
    let with_final: Vec<&TaskFact> = chosen_facts
        .iter()
        .copied()
        .filter(|fact| fact.final_insertions.is_some())
        .collect();
    let distributions = json!({
        "execution_ms": distribution(&column(&chosen_facts, |f| f.execution_ms.map(to_f64))),
        "queue_ms": distribution(&column(&chosen_facts, |f| f.queue_wait_ms.map(to_f64))),
        "elapsed_ms": distribution(&column(&chosen_facts, |f| f.elapsed_ms.map(to_f64))),
        "attempts": distribution(&column(&chosen_facts, |f| Some(to_f64(f.attempts)))),
        "files_changed": distribution(&column(&chosen_facts, |f| Some(to_f64(f.files_touched)))),
        "cumulative_churn": distribution(&column(&with_commits, |f| {
            Some(to_f64(f.churn_insertions + f.churn_deletions))
        })),
        "final_change_size": distribution(&column(&with_final, |f| {
            Some(to_f64(
                f.final_insertions.unwrap_or(0) + f.final_deletions.unwrap_or(0),
            ))
        })),
    });
    let runs: Vec<&VerificationRun> = input
        .verification_runs
        .iter()
        .filter(|run| {
            run.task_id
                .as_deref()
                .is_some_and(|task| ids.contains(&task))
        })
        .collect();
    let passed = runs
        .iter()
        .filter(|run| run.status.as_deref() == Some("passed"))
        .count();
    let pass_rate = (!runs.is_empty()).then(|| round3(passed as f64 / runs.len() as f64));
    let agents = agent_recommendations(&input.attempts, &ids);
    json!({
        "status": "ok",
        "basis": basis,
        "sample_size": ids.len(),
        "method": request.method,
        "distributions": distributions,
        "verification": {
            "runs": runs.len(),
            "passed": passed,
            "pass_rate": pass_rate,
        },
        "agent_recommendations": agents.iter().map(agent_row).collect::<Vec<Value>>(),
        "uncertainty": UNCERTAINTY,
        "caveats": [SELECTION_CAVEAT],
    })
}

/// The overlap of two sorted distinct sets: `|a & b| / |a | b|`.
fn overlap_of<T: Ord>(a: &[T], b: &[T]) -> f64 {
    let shared = a
        .iter()
        .filter(|item| b.binary_search(item).is_ok())
        .count();
    let union = a.len() + b.len() - shared;
    shared as f64 / union as f64
}

/// One row of `basis`: the task, its rounded score and the reasons it scored.
fn basis_row(task: &Scored) -> Value {
    json!({
        "task_id": task.task_id,
        "key": task.key,
        "title": task.title,
        "score": round3(task.score),
        "reasons": task.reasons,
    })
}

fn agent_row(agent: &Agent) -> Value {
    json!({
        "provider": agent.provider,
        "model": agent.model,
        "attempts": agent.attempts,
        "succeeded": agent.succeeded,
        "success_rate": agent.success_rate,
        "median_duration_ms": agent.median_duration_ms,
    })
}

/// The agent groups of the top-level attempts of the chosen tasks, best success rate first, then
/// most attempts first.
fn agent_recommendations(attempts: &VectorSync<AttemptRow>, ids: &[&str]) -> Vec<Agent> {
    let groups = group_sorted(
        attempts
            .iter()
            .filter(|attempt| attempt.parent_attempt_id.is_none())
            .filter(|attempt| {
                attempt
                    .task_id
                    .as_deref()
                    .is_some_and(|task| ids.contains(&task))
            })
            .map(|attempt| ((attempt.provider.clone(), attempt.model.clone()), attempt)),
    );
    let agents: Vec<Agent> = groups
        .into_iter()
        .map(|((provider, model), rows)| {
            let attempts = rows.len();
            let succeeded = rows
                .iter()
                .filter(|row| row.status.as_deref() == Some("succeeded"))
                .count();
            let durations = stable_sort_by(
                rows.iter()
                    .filter_map(|row| row.duration_ms.map(to_f64))
                    .collect::<Vec<f64>>(),
                |a, b| a.total_cmp(b),
            );
            Agent {
                provider,
                model: model.unwrap_or_else(|| "(unspecified)".to_owned()),
                attempts,
                succeeded,
                success_rate: (attempts > 0).then(|| round3(succeeded as f64 / attempts as f64)),
                median_duration_ms: quantile_cont(&durations, 0.5),
            }
        })
        .collect();
    stable_sort_by(agents, |a, b| {
        b.success_rate
            .unwrap_or(0.0)
            .total_cmp(&a.success_rate.unwrap_or(0.0))
            .then_with(|| b.attempts.cmp(&a.attempts))
    })
}

/// The values of one column over the tasks; a NULL stays `None` and is left out of the distribution.
fn column(tasks: &[&TaskFact], value: impl Fn(&TaskFact) -> Option<f64>) -> Vec<Option<f64>> {
    tasks.iter().map(|task| value(task)).collect()
}

/// `distribution`: the count and the percentiles of the non-NULL values, and their extremes.
fn distribution(values: &[Option<f64>]) -> Value {
    let clean: Vec<f64> = values.iter().flatten().copied().collect();
    let count = clean.len();
    let sorted = stable_sort_by(clean, |a, b| a.total_cmp(b));
    json!({
        "n": count,
        "p25": percentile(&sorted, 0.25),
        "median": percentile(&sorted, 0.5),
        "p75": percentile(&sorted, 0.75),
        "p90": percentile(&sorted, 0.9),
        "min": sorted.first().copied(),
        "max": sorted.last().copied(),
    })
}

/// `percentile(values, q)` over ascending values, with Python's interpolation arithmetic.
fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let position = last as f64 * q;
    let low = position as usize;
    let high = (low + 1).min(last);
    let (below, above) = (*sorted.get(low)?, *sorted.get(high)?);
    Some(below + (above - below) * (position - low as f64))
}

/// An integer column as the float Python's `float(v)` gives.
fn to_f64(value: i64) -> f64 {
    value as f64
}
