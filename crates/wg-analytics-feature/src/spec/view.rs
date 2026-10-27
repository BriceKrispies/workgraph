//! Pure: the human views of `analytics metrics` and `analytics query` (`cli/commands/analytics.py`):
//! the Rich tables and lines a result prints, as functions of the result document and the console.

use wg_clispec::args::{get, items, python, s};
use wg_json::Value;
use wg_render::Console;

/// `table(rows, headers)` cell text: `None` is empty, anything else is `str()`.
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(other) => python(other),
    }
}

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
            let definition: String = s(get(d, "definition")).chars().take(80).collect();
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
