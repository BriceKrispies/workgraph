//! The human views of `experiment create, freeze, close, next, report, fit, predict`: the lines
//! and tables the Python CLI prints, built from the JSON values the services return.

use wg_clispec::args::{get, items, opt, python, s, when};
use wg_json::{Value, py_float_repr};
use wg_render::Console;

/// A float as Python's `str(float)`; `None` for null.
fn number_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Float(number)) => py_float_repr(*number),
        Some(other) => python(other),
        None => "None".to_owned(),
    }
}

fn float_of(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Float(number)) => Some(*number),
        Some(Value::Int(number)) => Some(*number as f64),
        _ => None,
    }
}

/// `format(x, ".{digits}f")`.
fn fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value.is_infinite() {
        if value < 0.0 { "-inf" } else { "inf" }.to_owned()
    } else {
        format!("{value:.digits$}")
    }
}

/// `common.kv` over owned values.
fn kv(console: &Console, pairs: &[(&str, String)], title: &str) -> String {
    let shown: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, Some(value.as_str())))
        .collect();
    console.kv(&shown, Some(title)).text
}

/// `experiment create`.
#[must_use]
pub fn create_view(console: &Console, experiment: &Value) -> String {
    let slug = s(get(experiment, "slug"));
    kv(
        console,
        &[
            ("slug", slug.clone()),
            ("status", s(get(experiment, "status"))),
            ("id", s(get(experiment, "id"))),
        ],
        &format!("created experiment {slug}"),
    )
}

/// `experiment freeze`.
#[must_use]
pub fn freeze_view(console: &Console, experiment: &Value) -> String {
    let slug = s(get(experiment, "slug"));
    kv(
        console,
        &[
            ("slug", slug.clone()),
            ("status", s(get(experiment, "status"))),
            ("frozen", when(get(experiment, "frozen_at"))),
        ],
        &format!("frozen {slug}"),
    )
}

/// `experiment close`.
#[must_use]
pub fn close_view(console: &Console, experiment: &Value) -> String {
    let slug = s(get(experiment, "slug"));
    kv(
        console,
        &[
            ("slug", slug.clone()),
            ("status", s(get(experiment, "status"))),
        ],
        &format!("closed {slug}"),
    )
}

/// `_render_trial`: one trial.
fn trial_view(console: &Console, trial: &Value) -> String {
    let seq = s(get(trial, "seq"));
    let task = s(get(trial, "task_key"));
    let cause = opt(get(trial, "failure_cause")).filter(|cause| !cause.is_empty());
    let failure = cause
        .map(|cause| format!("{cause} ({})", s(get(trial, "failure_cause_source"))))
        .unwrap_or_default();
    let text = |name: &str| opt(get(trial, name)).unwrap_or_default();
    kv(
        console,
        &[
            ("trial", s(get(trial, "id"))),
            ("experiment", s(get(trial, "experiment_id"))),
            ("task", task.clone()),
            ("arm", s(get(trial, "arm"))),
            ("phase", s(get(trial, "phase"))),
            ("seq", seq.clone()),
            ("band", text("band")),
            ("requested model", s(get(trial, "requested_model"))),
            ("actual model", text("actual_model")),
            ("status", s(get(trial, "status"))),
            ("outcome", text("outcome")),
            ("failure cause", failure),
            ("attempt", text("attempt_id")),
            ("subagent run", text("subagent_run_id")),
            ("base ref", text("base_ref")),
            ("worktree", text("worktree_path")),
            ("notes", text("notes")),
        ],
        &format!("trial {seq} of {task}"),
    )
}

/// `_render_next`.
#[must_use]
pub fn next_view(console: &Console, plan: &Value) -> String {
    let reason = s(get(plan, "reason"));
    match get(plan, "trial") {
        None | Some(Value::Null) => console.print(&format!("done: {reason}")).text,
        Some(trial) => {
            let pending = if reason == "pending" {
                console
                    .print(&format!(
                        "pending: {} ({}) is not graded yet",
                        s(get(trial, "task_key")),
                        s(get(trial, "phase"))
                    ))
                    .text
            } else {
                String::new()
            };
            format!("{pending}{}", trial_view(console, trial))
        }
    }
}

/// `_fit_line`: the 50% and 80% sizes of a size fit.
fn fit_line(fit: &Value) -> String {
    let size = |value: Option<&Value>, interval: Option<&Value>| -> String {
        let Some(lines) = float_of(value) else {
            return "n/a".to_owned();
        };
        let bounds = match interval {
            Some(Value::List(pair)) if pair.len() == 2 => format!(
                " [{}, {}]",
                fixed(float_of(pair.first()).unwrap_or(f64::NAN), 0),
                fixed(float_of(pair.get(1)).unwrap_or(f64::NAN), 0)
            ),
            _ => " [n/a]".to_owned(),
        };
        format!("{} lines{bounds}", fixed(lines, 0))
    };
    format!(
        "50%: {}, 80%: {}",
        size(get(fit, "lines50"), get(fit, "ci50")),
        size(get(fit, "lines80"), get(fit, "ci80"))
    )
}

fn usage_line(outcome: &str, usage: &Value) -> String {
    let median = |name: &str| number_text(get(usage, name).and_then(|d| get(d, "median")));
    format!(
        "usage {outcome}: n={} missing={} output_tokens median={} turns median={} \
         tool_errors median={}",
        s(get(usage, "n")),
        s(get(usage, "missing_usage")),
        median("output_tokens"),
        median("turns"),
        median("tool_errors")
    )
}

/// One arm of `_render_report`.
fn arm_view(console: &Console, arm: &Value) -> String {
    let name = s(get(arm, "arm"));
    let heading = console.print(&format!("\n[bold]{name}[/bold]")).text;
    let rows: Vec<Vec<String>> = items(get(arm, "bands"))
        .iter()
        .map(|band| {
            let wilson = items(get(band, "wilson"));
            let bound = |at: usize| fixed(float_of(wilson.get(at)).unwrap_or(f64::NAN), 2);
            vec![
                s(get(band, "band")),
                s(get(band, "n")),
                s(get(band, "passes")),
                fixed(float_of(get(band, "rate")).unwrap_or(f64::NAN), 2),
                format!("[{}, {}]", bound(0), bound(1)),
            ]
        })
        .collect();
    let table = console
        .table(&rows, &["band", "n", "passes", "rate", "wilson 95%"], None)
        .text;
    let fit = match get(arm, "fit") {
        Some(Value::Null) | None => format!("fit: {}", s(get(arm, "fit_reason"))),
        Some(fit) => fit_line(fit),
    };
    let usage: String = match get(arm, "usage") {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(outcome, usage)| {
                console
                    .print(&usage_line(&python(&Value::Str(outcome.clone())), usage))
                    .text
            })
            .collect(),
        _ => String::new(),
    };
    let causes: Vec<String> = match get(arm, "failure_causes") {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(cause, n)| format!("{}={}", python(&Value::Str(cause.clone())), python(n)))
            .collect(),
        _ => Vec::new(),
    };
    let causes = if causes.is_empty() {
        "none".to_owned()
    } else {
        causes.join(", ")
    };
    let invalid: String = items(get(arm, "invalid"))
        .iter()
        .map(|item| {
            console
                .print(&format!(
                    "invalid: seq {} {} {}",
                    s(get(item, "seq")),
                    s(get(item, "task_key")),
                    opt(get(item, "failure_cause")).unwrap_or_default()
                ))
                .text
        })
        .collect();
    format!(
        "{heading}{table}{}{usage}{}{invalid}",
        console.print(&fit).text,
        console.print(&format!("failure causes: {causes}")).text
    )
}

/// `_render_report`.
#[must_use]
pub fn report_view(console: &Console, report: &Value) -> String {
    let experiment = get(report, "experiment");
    let step = get(report, "step");
    let band = match step.and_then(|step| get(step, "band")) {
        Some(value) => python(value),
        None => "None".to_owned(),
    };
    let head = console
        .print(&format!(
            "experiment {} ({}): phase {}, band {band}",
            experiment.map(|e| s(get(e, "slug"))).unwrap_or_default(),
            experiment.map(|e| s(get(e, "status"))).unwrap_or_default(),
            step.map(|step| s(get(step, "phase"))).unwrap_or_default(),
        ))
        .text;
    let arms: String = items(get(report, "arms"))
        .iter()
        .map(|arm| arm_view(console, arm))
        .collect();
    format!("{head}{arms}")
}

/// `_render_fit`.
#[must_use]
pub fn fit_view(console: &Console, fit: &Value) -> String {
    let metrics = get(fit, "metrics");
    let metric = |name: &str| metrics.and_then(|m| get(m, name));
    let brier = float_of(metric("brier")).unwrap_or(0.0);
    let count = |name: &str| metric(name).map_or_else(|| "0".to_owned(), python);
    let repositories: Vec<String> = items(get(fit, "repositories")).iter().map(python).collect();
    let repositories = if repositories.is_empty() {
        "none".to_owned()
    } else {
        repositories.join(", ")
    };
    let model = s(get(fit, "model"));
    kv(
        console,
        &[
            ("fit", s(get(fit, "id"))),
            ("model", model.clone()),
            ("data cutoff", when(get(fit, "data_cutoff"))),
            ("n", s(get(fit, "n"))),
            (
                "converged",
                if matches!(get(fit, "converged"), Some(Value::Bool(true))) {
                    "yes"
                } else {
                    "no"
                }
                .to_owned(),
            ),
            ("in-sample brier", fixed(brier, 4)),
            (
                "passes / fails",
                format!("{} / {}", count("n_pass"), count("n_fail")),
            ),
            ("repositories", repositories),
            (
                "bootstrap samples",
                items(get(fit, "samples")).len().to_string(),
            ),
        ],
        &format!("predictor fit for {model}"),
    )
}

/// `_render_prediction`.
#[must_use]
pub fn prediction_view(console: &Console, prediction: &Value) -> String {
    let interval = match (
        float_of(get(prediction, "low")),
        float_of(get(prediction, "high")),
    ) {
        (Some(low), Some(high)) => format!("95% {} to {}", fixed(low, 3), fixed(high, 3)),
        _ => "none (fewer than 20 bootstrap samples)".to_owned(),
    };
    let repository = opt(get(prediction, "repository"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "none".to_owned());
    let known = if matches!(get(prediction, "repository_known"), Some(Value::Bool(true))) {
        "yes"
    } else {
        "no (shared estimate)"
    };
    kv(
        console,
        &[
            (
                "probability",
                fixed(
                    float_of(get(prediction, "probability")).unwrap_or(f64::NAN),
                    3,
                ),
            ),
            ("interval", interval),
            ("repository", repository),
            ("repository in fit", known.to_owned()),
            ("fit", s(get(prediction, "fit_id"))),
            ("data cutoff", when(get(prediction, "data_cutoff"))),
            ("n", s(get(prediction, "n"))),
        ],
        &format!("{} success, inference", s(get(prediction, "model"))),
    )
}
