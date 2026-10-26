//! The human lines of the classification writes and of the search writes (`classify request`,
//! `confirm`, `promote`, `reclassify`, `gold`, `evaluate`, `search --reindex` and `--similar-to`),
//! as the Python commands print them. Each takes the reply the service built.

use wg_json::{PyStr, Value};

/// A Python string as a Rust string (code points that are not scalar values are dropped).
fn text_of(text: &PyStr) -> String {
    text.code_points()
        .iter()
        .filter_map(|point| char::from_u32(*point))
        .collect()
}

/// The member `name` of a dictionary or model value.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Dict(entries) => entries
            .iter()
            .find(|(key, _)| text_of(key) == name)
            .map(|(_, value)| value),
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value),
        _ => None,
    }
}

/// A member as Python's `str()` shows it: text as it is, `None` for null, a float the way Python
/// reprs it (`1.0`, not `1`).
fn shown(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Str(text)) => text_of(text),
        Some(Value::Float(number)) => format!("{number:?}"),
        Some(Value::Int(number)) => number.to_string(),
        Some(Value::Bool(flag)) => if *flag { "True" } else { "False" }.to_owned(),
        Some(other) => format!("{other:?}"),
    }
}

/// The first `count` characters of a member, as the listings cut them.
fn cut(value: Option<&Value>, count: usize) -> String {
    shown(value).chars().take(count).collect()
}

/// `classify request` (queued): the job id.
#[must_use]
pub fn queued_line(out: &Value) -> String {
    format!("queued classification job {}", shown(member(out, "job_id")))
}

/// `classify request --now` and `classify show`'s row: its status, category, confidence and
/// provider and model.
#[must_use]
pub fn now_line(out: &Value) -> String {
    format!(
        "{}: {} (confidence {}) via {}/{}",
        shown(member(out, "status")),
        shown(member(out, "category")),
        shown(member(out, "confidence")),
        shown(member(out, "provider")),
        shown(member(out, "model")),
    )
}

/// `classify confirm`.
#[must_use]
pub fn confirmed_line(out: &Value) -> String {
    format!("confirmed {}", shown(member(out, "category")))
}

/// `classify promote`.
#[must_use]
pub fn promoted_line(out: &Value) -> String {
    format!("{} is now current", shown(member(out, "category")))
}

/// `classify reclassify`: the batch and the number of jobs queued.
#[must_use]
pub fn reclassified_line(out: &Value) -> String {
    format!(
        "batch {}: {} job(s) queued",
        shown(member(out, "batch_id")),
        shown(member(out, "tasks")),
    )
}

/// `classify gold --category`.
#[must_use]
pub fn gold_added_line(out: &Value) -> String {
    format!("gold label {} added", shown(member(out, "id")))
}

/// `search --reindex`.
#[must_use]
pub fn reindex_line(out: &Value) -> String {
    format!("queued {}", shown(member(out, "enqueued")))
}

/// `classify evaluate`: the model, the sample size, the failures, the accuracy and the macro F1.
#[must_use]
pub fn evaluation_lines(out: &Value) -> String {
    [
        format!(
            "model: {}/{}",
            shown(member(out, "provider")),
            shown(member(out, "model")),
        ),
        format!("samples: {}", shown(member(out, "sample_size"))),
        format!("failures: {}", shown(member(out, "failures"))),
        format!("accuracy: {}", shown(member(out, "accuracy"))),
        format!("macro F1: {}", shown(member(out, "macro_f1"))),
    ]
    .join("\n")
}

/// `search --similar-to`: one line per similar task, with its score, status, category, title and
/// the number of linked commits.
#[must_use]
pub fn similar_lines(out: &Value) -> String {
    let header = format!("similar ({})", shown(member(out, "method")));
    let rows = match member(out, "similar") {
        Some(Value::List(items)) => items
            .iter()
            .map(|item| {
                let commits = match member(item, "implementation") {
                    Some(Value::List(commits)) => commits.len(),
                    _ => 0,
                };
                let score = match member(item, "score") {
                    Some(Value::Float(number)) => format!("{number:.3}"),
                    other => shown(other),
                };
                format!(
                    "{}  {}  {}  {}  {}  {}",
                    shown(member(item, "key")),
                    score,
                    shown(member(item, "status")),
                    cut(member(item, "category"), 60),
                    cut(member(item, "title"), 60),
                    commits,
                )
            })
            .collect::<Vec<String>>(),
        _ => Vec::new(),
    };
    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<String>>()
        .join("\n")
}
