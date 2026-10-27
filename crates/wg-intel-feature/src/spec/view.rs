//! The human views of the `classify` reads and of `search` (`cli/commands/intelligence.py`): the
//! lines and tables the commands print, built from the values the service returns.

use wg_clispec::args::{cell, get, items, opt, python, s};
use wg_json::Value;
use wg_render::Console;
use wg_types::pyval::prefix;

/// `short(value)`: the first eight characters of an id.
fn short(value: Option<&Value>) -> String {
    prefix(&s(value), 8)
}

/// An f-string of a value, where a missing value prints as `None`.
fn fstr(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(other) => python(other),
    }
}

fn flag(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

/// `classify show`: the current classification as key/value lines, then the history table.
#[must_use]
pub fn show_view(console: &Console, result: &Value) -> String {
    let mut out = String::new();
    match get(result, "current").filter(|value| !matches!(value, Value::Null)) {
        Some(current) => {
            let pairs = [
                ("category", opt(get(current, "category"))),
                ("subtype", opt(get(current, "subtype"))),
                ("domain", opt(get(current, "domain"))),
                ("complexity", opt(get(current, "complexity"))),
                ("confidence", opt(get(current, "confidence"))),
                ("source", opt(get(current, "source"))),
                (
                    "model",
                    Some(format!(
                        "{}/{}",
                        fstr(get(current, "provider")),
                        fstr(get(current, "model"))
                    )),
                ),
                (
                    "prompt/taxonomy",
                    Some(format!(
                        "{}/{}",
                        fstr(get(current, "prompt_version")),
                        fstr(get(current, "taxonomy_version"))
                    )),
                ),
                ("rationale", opt(get(current, "rationale"))),
            ];
            let title = format!("{} (current)", s(get(result, "task_key")));
            let refs: Vec<(&str, Option<&str>)> = pairs
                .iter()
                .map(|(key, value)| (*key, value.as_deref()))
                .collect();
            out.push_str(&console.kv(&refs, Some(&title)).text);
        }
        None => out.push_str(&console.print("no current classification").text),
    }
    let rows: Vec<Vec<String>> = items(get(result, "history"))
        .iter()
        .map(|h| {
            vec![
                short(get(h, "id")),
                cell(get(h, "source")),
                cell(get(h, "status")),
                cell(get(h, "category")),
                cell(get(h, "provider")),
                cell(get(h, "model")),
                if flag(get(h, "is_current")) { "*" } else { "" }.to_owned(),
                cell(get(h, "validation_status")),
            ]
        })
        .collect();
    out.push_str(
        &console
            .table(
                &rows,
                &[
                    "id",
                    "source",
                    "status",
                    "category",
                    "provider",
                    "model",
                    "cur",
                    "validation",
                ],
                None,
            )
            .text,
    );
    out.push_str(
        &console
            .print(&format!("[dim]{}[/dim]", s(get(result, "note"))))
            .text,
    );
    out
}

/// `classify gold`: the labels as a table of id, category and title.
#[must_use]
pub fn gold_view(console: &Console, rows: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|g| {
            vec![
                short(get(g, "id")),
                cell(get(g, "category")),
                prefix(&s(get(g, "title")), 70),
            ]
        })
        .collect();
    console
        .table(&rows, &["id", "category", "title"], None)
        .text
}

/// `classify evaluations`: one row per evaluation.
#[must_use]
pub fn evaluations_view(console: &Console, rows: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|r| {
            vec![
                cell(get(r, "provider")),
                cell(get(r, "model")),
                cell(get(r, "prompt_version")),
                cell(get(r, "sample_size")),
                cell(get(r, "accuracy")),
                cell(get(r, "macro_f1")),
                prefix(&s(get(r, "created_at")), 16),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "provider", "model", "prompt", "n", "accuracy", "macro F1", "when",
            ],
            None,
        )
        .text
}

/// `classify disagreements`: one row per task, its classifications listed.
#[must_use]
pub fn disagreements_view(console: &Console, rows: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|r| {
            let parts: Vec<String> = items(get(r, "classifications"))
                .iter()
                .map(|c| {
                    format!(
                        "{}({}:{})",
                        fstr(get(c, "category")),
                        s(get(c, "source")),
                        fstr(get(c, "model"))
                    )
                })
                .collect();
            vec![s(get(r, "task_key")), parts.join(", ")]
        })
        .collect();
    console
        .table(&rows, &["task", "classifications"], None)
        .text
}

/// The keys of a dictionary value, in order, as text.
fn dict_keys(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(key, _)| {
                key.code_points()
                    .iter()
                    .filter_map(|point| char::from_u32(*point))
                    .collect()
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `classify evaluate`: the evaluation as key/value lines, then the confusion matrix with the
/// expected labels down and the predicted labels across.
#[must_use]
pub fn evaluation_view(console: &Console, result: &Value) -> String {
    let model = format!(
        "{}/{}",
        fstr(get(result, "provider")),
        fstr(get(result, "model"))
    );
    let pairs = [
        ("model", Some(model)),
        ("samples", opt(get(result, "sample_size"))),
        ("failures", opt(get(result, "failures"))),
        ("accuracy", opt(get(result, "accuracy"))),
        ("macro F1", opt(get(result, "macro_f1"))),
    ];
    let refs: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, value.as_deref()))
        .collect();
    let mut out = console.kv(&refs, Some("evaluation")).text;
    let confusion = get(result, "confusion");
    let labels = dict_keys(confusion);
    let rows: Vec<Vec<String>> = labels
        .iter()
        .map(|expected| {
            let row = confusion.and_then(|matrix| get(matrix, expected));
            std::iter::once(expected.clone())
                .chain(labels.iter().map(|predicted| {
                    row.and_then(|counts| get(counts, predicted))
                        .map_or_else(|| "0".to_owned(), python)
                }))
                .collect()
        })
        .collect();
    let headers: Vec<&str> = std::iter::once("expected \\ predicted")
        .chain(labels.iter().map(String::as_str))
        .collect();
    out.push_str(&console.table(&rows, &headers, None).text);
    out
}

/// `search --similar-to`: one row per similar task, its score, status, category, title and linked
/// commit count, titled with the method.
#[must_use]
pub fn similar_view(console: &Console, result: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(result, "similar"))
        .iter()
        .map(|entry| {
            let score = match get(entry, "score") {
                Some(Value::Float(score)) => format!("{score:.3}"),
                other => cell(other),
            };
            vec![
                s(get(entry, "key")),
                score,
                cell(get(entry, "status")),
                cell(get(entry, "category")),
                prefix(&s(get(entry, "title")), 60),
                items(get(entry, "implementation")).len().to_string(),
            ]
        })
        .collect();
    let title = format!("similar ({})", s(get(result, "method")));
    console
        .table(
            &rows,
            &["task", "score", "status", "category", "title", "commits"],
            Some(&title),
        )
        .text
}

/// `search`: the hits as a table titled with the mode, and the note under it.
#[must_use]
pub fn search_view(console: &Console, result: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(result, "hits"))
        .iter()
        .map(|h| {
            let score = match get(h, "score") {
                Some(Value::Float(score)) => format!("{score:.4}"),
                other => cell(other),
            };
            vec![
                s(get(h, "key")),
                score,
                s(get(h, "status")),
                prefix(&s(get(h, "title")), 60),
                prefix(&s(get(h, "snippet")), 60),
            ]
        })
        .collect();
    let title = format!("{} search", s(get(result, "mode")));
    let mut out = console
        .table(
            &rows,
            &["task", "score", "status", "title", "snippet"],
            Some(&title),
        )
        .text;
    if let Some(note) = opt(get(result, "note")) {
        out.push_str(&console.print(&format!("[dim]{note}[/dim]")).text);
    }
    out
}
