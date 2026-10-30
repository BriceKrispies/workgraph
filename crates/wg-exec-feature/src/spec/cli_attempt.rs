//! The pure half of the `attempt` read commands (`cli/commands/agents.py`): the clap command of
//! the group and the human views of the results. The views print what the Python tables and
//! key-value blocks print, cell for cell.

use clap::Command;
use wg_iface::cli::Snapshot;
use wg_iface::cli::args::{cell, get, items, python, s, when};
use wg_iface::cli::tree::{SNAPSHOT_JSON, placeholder};
use wg_json::Value;
use wg_render::Console;
use wg_types::pyval::prefix;

/// The clap command of the `attempt` group: the snapshot's own tree.
#[must_use]
pub fn command() -> Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "attempt"),
        Err(_) => wg_iface::cli::tree::command("attempt"),
    }
}

/// `_fmt_ms`: seconds with one decimal, or nothing.
fn fmt_ms(value: Option<&Value>) -> String {
    match value {
        Some(Value::Int(ms)) => format!("{:.1}s", *ms as f64 / 1000.0),
        Some(Value::Float(ms)) => format!("{:.1}s", ms / 1000.0),
        _ => String::new(),
    }
}

/// `short(id)`: the first eight characters of an id.
fn short(value: Option<&Value>) -> String {
    let text = cell(value);
    prefix(&text, 8)
}

/// The human view of `attempt list`: the page's items as a table.
#[must_use]
pub fn list_view(console: &Console, page: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(page, "items"))
        .iter()
        .map(|a| {
            vec![
                short(get(a, "id")),
                short(get(a, "task_id")),
                cell(get(a, "number")),
                cell(get(a, "status")),
                cell(get(a, "provider")),
                cell(get(a, "model")),
                fmt_ms(get(a, "duration_ms")),
                fmt_ms(get(a, "queue_wait_ms")),
                cell(get(a, "exit_reason")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "attempt", "task", "#", "status", "provider", "model", "duration", "queued", "exit",
            ],
            None,
        )
        .text
}

/// A key-value entry that is shown when the value is present; empty lists and dicts are left
/// out, as `common.kv` does.
fn shown(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::List(members)) if members.is_empty() => None,
        Some(Value::Dict(entries)) if entries.is_empty() => None,
        Some(other) => Some(python(other)),
    }
}

/// `x.usage or "not measured"`: an empty or missing usage reads as not measured.
fn usage_text(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => Some("not measured".to_owned()),
        Some(Value::Dict(entries)) if entries.is_empty() => Some("not measured".to_owned()),
        Some(other) => Some(python(other)),
    }
}

/// `cost_usd if not None else "not measured"`.
fn cost_text(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => Some("not measured".to_owned()),
        Some(other) => Some(python(other)),
    }
}

/// The human view of `attempt show`: a titled key-value block.
#[must_use]
pub fn show_view(console: &Console, attempt: &Value) -> String {
    let queued = Some(when(get(attempt, "queued_at")));
    let started = Some(when(get(attempt, "started_at")));
    let finished = Some(when(get(attempt, "finished_at")));
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("id", shown(get(attempt, "id"))),
        ("task", shown(get(attempt, "task_id"))),
        ("number", shown(get(attempt, "number"))),
        ("status", shown(get(attempt, "status"))),
        ("provider", shown(get(attempt, "provider"))),
        ("adapter", shown(get(attempt, "adapter"))),
        ("model", shown(get(attempt, "model"))),
        ("session", shown(get(attempt, "session_id"))),
        ("parent", shown(get(attempt, "parent_attempt_id"))),
        ("retry of", shown(get(attempt, "retry_of_attempt_id"))),
        ("rerun of", shown(get(attempt, "rerun_of_attempt_id"))),
        ("queued", queued),
        ("started", started),
        ("finished", finished),
        ("duration", Some(fmt_ms(get(attempt, "duration_ms")))),
        ("queue wait", Some(fmt_ms(get(attempt, "queue_wait_ms")))),
        ("exit", shown(get(attempt, "exit_reason"))),
        ("error", shown(get(attempt, "error"))),
        ("usage", usage_text(get(attempt, "usage"))),
        ("usage source", shown(get(attempt, "usage_source"))),
        ("cost", cost_text(get(attempt, "cost_usd"))),
        (
            "modified repos",
            shown(get(attempt, "modified_repository_ids")),
        ),
        ("telemetry", shown(get(attempt, "telemetry"))),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    console.kv(&borrowed, Some("attempt")).text
}

/// The human view of `attempt manifests`: one table per manifest, titled by its kind.
#[must_use]
pub fn manifests_view(console: &Console, manifests: &Value) -> String {
    let mut out = String::new();
    for manifest in items(Some(manifests)) {
        let rows: Vec<Vec<String>> = items(get(manifest, "entries"))
            .iter()
            .map(|entry| {
                vec![
                    cell(get(entry, "repository")),
                    sha_cell(get(entry, "sha")),
                    cell(get(entry, "access")),
                    cell(get(entry, "checkout_path")),
                ]
            })
            .collect();
        let title = cell(get(manifest, "kind"));
        out.push_str(
            &console
                .table(
                    &rows,
                    &["repository", "sha", "access", "path"],
                    Some(&title),
                )
                .text,
        );
    }
    out
}

/// `e.get("sha", "")[:12] if e.get("sha") else ""`: a non-empty sha, cut to twelve characters.
fn sha_cell(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(_)) => prefix(&cell(value), 12),
        _ => String::new(),
    }
}

/// The human view of `attempt compare`: one row per compared field, marked DIFF when it differs.
#[must_use]
pub fn compare_view(console: &Console, compared: &Value) -> String {
    let rows: Vec<Vec<String>> = match get(compared, "fields") {
        Some(Value::Dict(fields)) => fields
            .iter()
            .map(|(name, field)| {
                let same = matches!(get(field, "same"), Some(Value::Bool(true)));
                vec![
                    python(&Value::Str(name.clone())),
                    cell(get(field, "a")),
                    cell(get(field, "b")),
                    if same {
                        String::new()
                    } else {
                        "DIFF".to_owned()
                    },
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console.table(&rows, &["field", "a", "b", ""], None).text
}

/// The human view of `attempt create`: `attempt <id> (#<n>) <status>`.
#[must_use]
pub fn create_line(console: &Console, attempt: &Value) -> String {
    console
        .print(&format!(
            "attempt {} (#{}) {}",
            cell(get(attempt, "id")),
            cell(get(attempt, "number")),
            cell(get(attempt, "status"))
        ))
        .text
}

/// The human view of `attempt finish`: `attempt <short id> <status>`.
#[must_use]
pub fn finish_line(console: &Console, attempt: &Value) -> String {
    console
        .print(&format!(
            "attempt {} {}",
            short(get(attempt, "id")),
            cell(get(attempt, "status"))
        ))
        .text
}

/// The human view of `attempt abandon`.
#[must_use]
pub fn abandon_line(console: &Console, out: &Value) -> String {
    console
        .print(&format!(
            "attempt {} abandoned; task {} {}",
            short(get(out, "attempt_id")),
            cell(get(out, "task")),
            cell(get(out, "task_status"))
        ))
        .text
}

/// The human view of `attempt start` (`_print_attempt_start`): the attempt, each checkout and the
/// fast checks.
#[must_use]
pub fn start_view(console: &Console, out: &Value) -> String {
    let head = console
        .print(&format!(
            "attempt {} (#{}) running on {}",
            short(get(out, "attempt_id")),
            cell(get(out, "attempt_number")),
            cell(get(out, "task_key"))
        ))
        .text;
    let repositories: String = items(get(out, "repositories"))
        .iter()
        .map(|repo| {
            let base = match get(repo, "base_sha") {
                Some(Value::Str(_)) => prefix(&cell(get(repo, "base_sha")), 12),
                _ => "?".to_owned(),
            };
            console
                .print(&format!(
                    "  {}: {} on {} (base {})",
                    s(get(repo, "repository")),
                    s(get(repo, "checkout_path")),
                    python_text(get(repo, "branch")),
                    base
                ))
                .text
        })
        .collect();
    let checks = items(get(out, "agent_checks"));
    let fast = if checks.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = checks.iter().map(|check| s(get(check, "name"))).collect();
        console
            .print(&format!("fast checks: {}", names.join(", ")))
            .text
    };
    format!("{head}{repositories}{fast}")
}

/// A value's Python `str()`, with a missing value as `None`.
fn python_text(value: Option<&Value>) -> String {
    value.map_or_else(|| "None".to_owned(), python)
}

/// The human view of `attempt submit` (`_print_attempt_submit`).
#[must_use]
pub fn submit_view(console: &Console, reference: &str, out: &Value) -> String {
    let status = cell(get(out, "status"));
    let suffix = if status == "succeeded" {
        String::new()
    } else {
        format!(" ({status})")
    };
    let head = console
        .print(&format!(
            "attempt {} {}; task {}{}",
            prefix(reference, 8),
            cell(get(out, "attempt_status")),
            cell(get(out, "task_status")),
            suffix
        ))
        .text;
    let verification = match get(out, "verification") {
        Some(value) if truthy(value) => {
            console
                .print(&format!("verification: {}", python(value)))
                .text
        }
        _ => String::new(),
    };
    let error = match get(out, "error") {
        Some(value) if truthy(value) => console.print(&format!("error: {}", python(value))).text,
        _ => String::new(),
    };
    format!("{head}{verification}{error}")
}

/// Python truthiness of an output value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Int(number) => *number != 0,
        Value::Float(number) => *number != 0.0,
        Value::List(members) => !members.is_empty(),
        Value::Dict(entries) => !entries.is_empty(),
        other => !python(other).is_empty(),
    }
}
