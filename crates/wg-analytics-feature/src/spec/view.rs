//! Pure: the human views of `analytics metrics` and `analytics query` (`cli/commands/analytics.py`):
//! the Rich tables and lines a result prints, as functions of the result document and the console.

use wg_clispec::args::{cell, get, items, python, s};
use wg_json::Value;
use wg_render::Console;
use wg_types::pyval::prefix;

/// The Python `table(...)` of a list of rows, followed by its text. Rich prints a table with
/// nothing in it as `nothing to show`, which the renderer does itself.
fn table(console: &Console, rows: Vec<Vec<String>>, headers: &[&str]) -> String {
    console.table(&rows, headers, None).text
}

/// A dict key as text.
fn key_text(key: &wg_json::PyStr) -> String {
    key.code_points()
        .iter()
        .filter_map(|code| char::from_u32(*code))
        .collect()
}

/// `metrics`: the catalog, one row per metric with its definition cut to 80 characters.
#[must_use]
pub fn metrics_view(console: &Console, definitions: &Value) -> String {
    let rows = items(Some(definitions))
        .iter()
        .map(|d| {
            let definition: String = prefix(&s(get(d, "definition")), 80);
            vec![
                cell(get(d, "id")),
                cell(get(d, "family")),
                cell(get(d, "statistic")),
                cell(get(d, "unit")),
                definition,
            ]
        })
        .collect();
    table(
        console,
        rows,
        &["metric", "family", "stat", "unit", "definition"],
    )
}

/// `query`: the metric's title and definition, the grouped rows (the `*_label` columns replace
/// their ids), `no data` when there are none, then the notes.
#[must_use]
pub fn query_view(console: &Console, result: &Value) -> String {
    let metric = get(result, "metric");
    let field = |name: &str| s(metric.and_then(|m| get(m, name)));
    let mut out = console
        .print(&format!(
            "[bold]{}[/bold] - {}",
            field("title"),
            field("definition")
        ))
        .text;
    let rows = items(get(result, "rows"));
    match rows.first() {
        Some(first) => {
            let columns: Vec<String> = match first {
                Value::Dict(entries) => entries
                    .iter()
                    .map(|(key, _)| key_text(key))
                    .filter(|name| !name.ends_with("_label"))
                    .collect(),
                _ => Vec::new(),
            };
            let body = rows
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|column| {
                            let label = format!("{column}_label");
                            cell(get(row, &label).or_else(|| get(row, column)))
                        })
                        .collect()
                })
                .collect();
            let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
            out.push_str(&table(console, body, &headers));
        }
        None => out.push_str(&console.print("[dim]no data[/dim]").text),
    }
    for note in items(get(result, "notes")) {
        out.push_str(
            &console
                .print(&format!("[dim]note: {}[/dim]", s(Some(note))))
                .text,
        );
    }
    out
}

/// `common.kv`: a pair whose value is `None`, `[]` or `{}` is left out; every other value prints as
/// Python's `str()` of it.
fn kv(console: &Console, pairs: &[(String, &Value)], title: Option<&str>) -> String {
    let shown: Vec<(&str, Option<String>)> = pairs
        .iter()
        .map(|(key, value)| {
            let empty = match value {
                Value::Null => true,
                Value::List(members) => members.is_empty(),
                Value::Dict(entries) => entries.is_empty(),
                _ => false,
            };
            (key.as_str(), (!empty).then(|| python(value)))
        })
        .collect();
    let borrowed: Vec<(&str, Option<&str>)> = shown
        .iter()
        .map(|(key, text)| (*key, text.as_deref()))
        .collect();
    console.kv(&borrowed, title).text
}

/// The members of a dict result, as `(key, value)` pairs.
fn pairs(value: Option<&Value>) -> Vec<(String, &Value)> {
    match value {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(key, member)| (key_text(key), member))
            .collect(),
        _ => Vec::new(),
    }
}

/// `overview`: one `key: value` line per section of the bundle.
#[must_use]
pub fn overview_view(console: &Console, result: &Value) -> String {
    kv(console, &pairs(Some(result)), None)
}

/// `compare`: the group columns the command was given (`period` has no column in a row), then
/// both values, the change and both sample sizes.
#[must_use]
pub fn compare_view(console: &Console, result: &Value, group_by: &[String]) -> String {
    let rows = items(get(result, "rows"))
        .iter()
        .map(|row| {
            group_by
                .iter()
                .map(|group| cell(get(row, group)))
                .chain(
                    ["a", "b", "delta", "n_a", "n_b"]
                        .iter()
                        .map(|name| cell(get(row, name))),
                )
                .collect()
        })
        .collect();
    let headers: Vec<&str> = group_by
        .iter()
        .map(String::as_str)
        .chain(["a", "b", "delta", "n_a", "n_b"])
        .collect();
    table(console, rows, &headers)
}

/// `coupling`: the coordination summary, the edge table and the notes.
#[must_use]
pub fn coupling_view(console: &Console, result: &Value) -> String {
    let summary = kv(
        console,
        &pairs(get(result, "summary")),
        Some("coordination"),
    );
    let rows = items(get(result, "edges"))
        .iter()
        .map(|edge| {
            let evidence: Vec<String> = items(get(edge, "evidence"))
                .iter()
                .take(5)
                .map(|ev| s(get(ev, "task_key")))
                .collect();
            vec![
                cell(get(edge, "source_name")),
                cell(get(edge, "target_name")),
                cell(get(edge, "co_changes")),
                cell(get(edge, "jaccard")),
                cell(get(edge, "relationship")),
                evidence.join(", "),
            ]
        })
        .collect();
    let edges = table(
        console,
        rows,
        &[
            "repository",
            "repository",
            "co-changes",
            "jaccard",
            "relationship",
            "evidence",
        ],
    );
    let notes: String = items(get(result, "notes"))
        .iter()
        .map(|note| console.print(&format!("[dim]{}[/dim]", s(Some(note)))).text)
        .collect();
    format!("{summary}{edges}{notes}")
}

/// `failures`: the findings table.
#[must_use]
pub fn failures_view(console: &Console, result: &Value) -> String {
    let rows = items(get(result, "findings"))
        .iter()
        .map(|finding| {
            vec![
                cell(get(finding, "kind")),
                cell(get(finding, "severity")),
                cell(get(finding, "title")),
                cell(get(finding, "sample_size")),
            ]
        })
        .collect();
    table(console, rows, &["kind", "severity", "finding", "n"])
}

/// `(a or 0) + (b or 0)` of two optional counts.
fn total(row: &Value, first: &str, second: &str) -> String {
    let count = |name: &str| match get(row, name) {
        Some(Value::Int(number)) => *number,
        _ => 0,
    };
    (count(first) + count(second)).to_string()
}

/// `history`: one row per accepted task.
#[must_use]
pub fn history_view(console: &Console, rows: &Value) -> String {
    let body = match rows {
        Value::List(members) => members
            .iter()
            .map(|row| {
                vec![
                    cell(get(row, "key")),
                    cell(get(row, "category")),
                    cell(get(row, "attempts")),
                    cell(get(row, "commits")),
                    total(row, "churn_insertions", "churn_deletions"),
                    total(row, "final_insertions", "final_deletions"),
                    prefix(&s(get(row, "title")), 50),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    table(
        console,
        body,
        &[
            "task", "category", "attempts", "commits", "churn", "final", "title",
        ],
    )
}

/// `estimate`: the sample and its method, the distributions, the comparable tasks and the agents on
/// similar tasks; `insufficient_data` prints its message alone.
#[must_use]
pub fn estimate_view(console: &Console, result: &Value) -> String {
    if s(get(result, "status")) != "ok" {
        return console
            .print(&format!("[yellow]{}[/yellow]", s(get(result, "message"))))
            .text;
    }
    let mut out = console
        .print(&format!(
            "based on {} comparable tasks ({} similarity)",
            s(get(result, "sample_size")),
            s(get(result, "method"))
        ))
        .text;
    let distributions = get(result, "distributions").map(|value| match value {
        // Each measure of the distributions document, with its count and percentiles.
        Value::Dict(entries) => entries
            .iter()
            .map(|(name, d)| {
                vec![
                    key_text(name),
                    cell(get(d, "n")),
                    cell(get(d, "p25")),
                    cell(get(d, "median")),
                    cell(get(d, "p75")),
                    cell(get(d, "p90")),
                ]
            })
            .collect(),
        _ => Vec::new(),
    });
    out.push_str(&table(
        console,
        distributions.unwrap_or_default(),
        &["measure", "n", "p25", "median", "p75", "p90"],
    ));
    let basis = items(get(result, "basis"))
        .iter()
        .map(|task| {
            let why: Vec<String> = items(get(task, "reasons"))
                .iter()
                .map(|reason| s(Some(reason)))
                .collect();
            vec![
                cell(get(task, "key")),
                cell(get(task, "score")),
                why.join("; "),
            ]
        })
        .collect();
    out.push_str(&table(console, basis, &["task", "score", "why"]));
    let agents: Vec<Vec<String>> = items(get(result, "agent_recommendations"))
        .iter()
        .map(|agent| {
            vec![
                cell(get(agent, "provider")),
                cell(get(agent, "model")),
                cell(get(agent, "attempts")),
                cell(get(agent, "success_rate")),
            ]
        })
        .collect();
    out.push_str(
        &console
            .table(
                &agents,
                &["provider", "model", "attempts", "success"],
                Some("agents on similar tasks (selection bias applies)"),
            )
            .text,
    );
    out
}

/// `retrospective`: the report text, printed as it is (Python `console.print(content, markup=False)`).
#[must_use]
pub fn retrospective_view(console: &Console, result: &Value) -> String {
    console
        .print(&wg_render::escape(&s(get(result, "content"))))
        .text
}
