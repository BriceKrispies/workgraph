//! The human views of the `classify` reads and of `search` (`cli/commands/intelligence.py`): the
//! lines and tables the commands print, built from the values the service returns.

use wg_clispec::args::{get, items, opt, python, s, truncate};
use wg_json::Value;
use wg_render::Console;

/// `short(value)`: the first eight characters of an id.
fn short(value: Option<&Value>) -> String {
    truncate(&s(value), 8)
}

/// A cell of a table: `""` for a missing value, `str()` of the rest.
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(other) => python(other),
    }
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
                truncate(&s(get(g, "title")), 70),
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
                truncate(&s(get(r, "created_at")), 16),
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
                truncate(&s(get(h, "title")), 60),
                truncate(&s(get(h, "snippet")), 60),
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
