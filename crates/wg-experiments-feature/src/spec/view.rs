//! The human views of the `experiment` commands (IF-19): the lines and tables the Python CLI
//! prints, built from the JSON values the service returns.

use wg_clispec::args::{get, items, opt, s, truncate, when};
use wg_json::Value;
use wg_render::Console;

/// The columns of `experiment list`.
const LIST_HEAD: [&str; 4] = ["slug", "status", "frozen", "title"];

/// The columns of the trial table of `experiment show`.
const TRIAL_HEAD: [&str; 9] = [
    "seq", "task", "arm", "phase", "band", "status", "outcome", "cause", "model",
];

/// The human view of `experiment list`: one row per experiment, or the dim "nothing to show".
#[must_use]
pub fn list_view(console: &Console, experiments: &Value) -> String {
    let rows: Vec<Vec<String>> = match experiments {
        Value::List(list) => list.iter().map(list_row).collect(),
        _ => Vec::new(),
    };
    console.table(&rows, &LIST_HEAD, None).text
}

fn list_row(experiment: &Value) -> Vec<String> {
    vec![
        s(get(experiment, "slug")),
        s(get(experiment, "status")),
        when(get(experiment, "frozen_at")),
        s(get(experiment, "title")),
    ]
}

/// The human view of `experiment show`: the protocol summary, then the trials in order.
#[must_use]
pub fn show_view(console: &Console, shown: &Value) -> String {
    let Some(experiment) = get(shown, "experiment") else {
        return String::new();
    };
    let slug = s(get(experiment, "slug"));
    let frozen = when(get(experiment, "frozen_at"));
    let frozen = if frozen.is_empty() {
        "not frozen".to_owned()
    } else {
        frozen
    };
    let question = opt(get(experiment, "question")).unwrap_or_default();
    let title = s(get(experiment, "title"));
    let status = s(get(experiment, "status"));
    let id = s(get(experiment, "id"));
    let heading = format!("experiment {slug}");
    let pairs: [(&str, Option<&str>); 6] = [
        ("slug", Some(slug.as_str())),
        ("title", Some(title.as_str())),
        ("status", Some(status.as_str())),
        ("question", Some(question.as_str())),
        ("frozen", Some(frozen.as_str())),
        ("id", Some(id.as_str())),
    ];
    let header = console.kv(&pairs, Some(&heading)).text;
    let rows: Vec<Vec<String>> = items(get(shown, "trials")).iter().map(trial_row).collect();
    format!("{header}{}", console.table(&rows, &TRIAL_HEAD, None).text)
}

/// One trial row: `seq task arm phase band status outcome cause model`, where the model is the
/// actual model or, when that is empty, the requested one, cut to 24 characters.
fn trial_row(trial: &Value) -> Vec<String> {
    let model = opt(get(trial, "actual_model"))
        .filter(|model| !model.is_empty())
        .or_else(|| opt(get(trial, "requested_model")))
        .unwrap_or_default();
    vec![
        s(get(trial, "seq")),
        s(get(trial, "task_key")),
        s(get(trial, "arm")),
        s(get(trial, "phase")),
        opt(get(trial, "band")).unwrap_or_default(),
        s(get(trial, "status")),
        opt(get(trial, "outcome")).unwrap_or_default(),
        opt(get(trial, "failure_cause")).unwrap_or_default(),
        truncate(&model, 24),
    ]
}
