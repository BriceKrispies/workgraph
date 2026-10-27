//! The human views of the `task` commands: the lines and tables `task` prints, built from the
//! JSON values the service returns.

use wg_clispec::args::count;
use wg_clispec::args::{get, items, opt, python, s, when};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_render::{Console, escape};
use wg_types::WgError;
use wg_types::pyval::prefix;

/// `emit(out, ...)` of `task create`.
#[must_use]
pub fn created_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "created [bold]{}[/bold] ({}) {}",
            s(get(task, "key")),
            s(get(task, "status")),
            s(get(task, "title"))
        ))
        .text
}

/// The human view of `task list`.
#[must_use]
pub fn list_view(console: &Console, page: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(page, "items")).iter().map(task_row).collect();
    let mut out = console.table(&rows, &HEAD, None).text;
    let next =
        opt(get(page, "next_offset")).map_or_else(String::new, |n| format!("; next: --offset {n}"));
    out.push_str(
        &console
            .print(&format!(
                "[dim]{} of {}{next}[/dim]",
                rows.len(),
                s(get(page, "total"))
            ))
            .text,
    );
    out
}

/// The human view of `task ready`.
#[must_use]
pub fn ready_view(console: &Console, tasks: &Value) -> String {
    let rows: Vec<Vec<String>> = match tasks {
        Value::List(list) => list.iter().map(task_row).collect(),
        _ => Vec::new(),
    };
    console.table(&rows, &HEAD, Some("ready")).text
}

/// `_render_task`.
#[must_use]
pub fn task_view(console: &Console, task: &Value) -> String {
    let tags: Vec<String> = items(get(task, "tags")).iter().map(python).collect();
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("id", opt(get(task, "id"))),
        ("title", opt(get(task, "title"))),
        ("status", opt(get(task, "status"))),
        ("priority", opt(get(task, "priority"))),
        ("category", opt(get(task, "category"))),
        ("tags", Some(tags.join(", "))),
        ("assignee", opt(get(task, "assignee"))),
        ("claimed by", opt(get(task, "claimed_by"))),
        ("parent", opt(get(task, "parent_id"))),
        ("children", count(get(task, "child_ids"))),
        ("depends on", count(get(task, "depends_on"))),
        ("blocks", count(get(task, "blocks"))),
        ("projects", count(get(task, "project_ids"))),
        ("repositories", count(get(task, "repository_ids"))),
        ("files", count(get(task, "files"))),
        ("version", opt(get(task, "version"))),
        ("created", Some(when(get(task, "created_at")))),
        ("completed", Some(when(get(task, "completed_at")))),
        ("accepted", Some(when(get(task, "accepted_at")))),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    let key = s(get(task, "key"));
    let mut out = console.kv(&borrowed, Some(&key)).text;
    let description = s(get(task, "description"));
    if !description.is_empty() {
        out.push_str(&console.print("").text);
        out.push_str(&console.print(&description).text);
    }
    let criteria = items(get(task, "acceptance_criteria"));
    if !criteria.is_empty() {
        out.push_str(&console.print("\n[bold]Acceptance criteria[/bold]").text);
        for item in criteria {
            out.push_str(&console.print(&format!("  - {}", python(item))).text);
        }
    }
    let files = items(get(task, "files"));
    if !files.is_empty() {
        out.push_str(&console.print("\n[bold]Files[/bold]").text);
        for file in files {
            let path = s(get(file, "path"));
            let label = match opt(get(file, "anchor")).filter(|a| !a.is_empty()) {
                Some(anchor) => format!("{path}:{anchor}"),
                None => path,
            };
            let note = opt(get(file, "note"))
                .filter(|n| !n.is_empty())
                .map_or_else(String::new, |n| format!(" - {}", escape(&n)));
            out.push_str(&console.print(&format!("  - {}{note}", escape(&label))).text);
        }
    }
    if let Some(rollup) = get(task, "rollup").filter(|r| !matches!(r, Value::Null)) {
        let accepted = s(get(rollup, "accepted"));
        let total = s(get(rollup, "total_descendants"));
        let progress = match get(rollup, "progress") {
            Some(Value::Float(p)) => format!("{:.0}%", p * 100.0),
            _ => String::new(),
        };
        let by_status = match get(rollup, "by_status") {
            Some(Value::Dict(entries)) => {
                let inner: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}: {}",
                            py_repr_str(&python(&Value::Str(k.clone()))),
                            python(v)
                        )
                    })
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            _ => String::new(),
        };
        out.push_str(
            &console
                .print(&format!(
                    "\n[bold]Rollup[/bold]: {accepted}/{total} accepted ({progress}) {by_status}"
                ))
                .text,
        );
    }
    if let Some(state) = get(task, "dependency_state")
        && matches!(get(state, "satisfied"), Some(Value::Bool(false)))
    {
        let keys: Vec<String> = items(get(state, "unsatisfied"))
            .iter()
            .map(|b| s(get(b, "key")))
            .collect();
        out.push_str(
            &console
                .print(&format!("[yellow]Blocked by:[/yellow] {}", keys.join(", ")))
                .text,
        );
    }
    let comments = items(get(task, "comments"));
    for comment in &comments[comments.len().saturating_sub(10)..] {
        out.push_str(
            &console
                .print(&format!(
                    "\n[dim]{} {} ({})[/dim]\n{}",
                    when(get(comment, "created_at")),
                    s(get(comment, "author")),
                    s(get(comment, "kind")),
                    s(get(comment, "body"))
                ))
                .text,
        );
    }
    out
}

/// The human view of `task show` with several references: standard output and standard error.
#[must_use]
pub fn shown_view(
    out: &Console,
    err: &Console,
    found: &[Value],
    errors: &[(String, WgError)],
) -> (String, String) {
    let mut stdout = String::new();
    for (index, task) in found.iter().enumerate() {
        if index > 0 {
            stdout.push_str(&out.print("").text);
        }
        stdout.push_str(&task_view(out, task));
    }
    let stderr = errors
        .iter()
        .map(|(reference, error)| {
            err.print(&format!(
                "[red]error[/red] ({}) {}: {}",
                error.code.as_str(),
                escape(reference),
                escape(&error.message)
            ))
            .text
        })
        .collect();
    (stdout, stderr)
}

/// The human view of `task update`.
#[must_use]
pub fn updated_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} updated (v{}, {})",
            s(get(task, "key")),
            s(get(task, "version")),
            s(get(task, "status"))
        ))
        .text
}

/// The human view of `task bulk-update`.
#[must_use]
pub fn bulk_view(console: &Console, result: &Value) -> String {
    let updated = items(get(result, "updated")).len();
    let (failed, shown) = match get(result, "failed") {
        Some(Value::Dict(entries)) if !entries.is_empty() => {
            let inner: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}: {}",
                        py_repr_str(&python(&Value::Str(k.clone()))),
                        py_repr_str(&python(v))
                    )
                })
                .collect();
            (entries.len(), format!("{{{}}}", inner.join(", ")))
        }
        _ => (0, String::new()),
    };
    console
        .print(&format!(
            "updated {updated}; failed {failed} {}",
            escape(&shown)
        ))
        .text
}

/// The human view of `task archive`.
#[must_use]
pub fn archived_line(console: &Console, task: &Value, undo: bool) -> String {
    console
        .print(&format!(
            "{} {}",
            s(get(task, "key")),
            if undo { "unarchived" } else { "archived" }
        ))
        .text
}

/// The human view of `task claim`.
#[must_use]
pub fn claimed_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "claimed {} until {}",
            s(get(task, "key")),
            when(get(task, "claim_expires_at"))
        ))
        .text
}

/// The human view of `task release`.
#[must_use]
pub fn released_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!("released {}", s(get(task, "key"))))
        .text
}

/// The human view of `task accept`; with `--promote-dependents` the output also names the
/// dependents it moved to ready.
#[must_use]
pub fn accepted_line(console: &Console, out: &Value, promote: bool) -> String {
    let base = format!(
        "accepted ({}) by {}",
        s(get(out, "mode")),
        s(get(out, "actor"))
    );
    let line = if promote {
        let promoted: Vec<String> = items(get(out, "promoted"))
            .iter()
            .map(|key| s(Some(key)))
            .collect();
        let names = if promoted.is_empty() {
            "no dependents".to_owned()
        } else {
            promoted.join(", ")
        };
        format!("{base}; promoted {names}")
    } else {
        base
    };
    console.print(&line).text
}

/// The human view of `task reject`.
#[must_use]
pub fn rejected_line(console: &Console) -> String {
    console.print("rejected").text
}

/// The human view of `task complete` and `task result`.
#[must_use]
pub fn status_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} is now {}",
            s(get(task, "key")),
            s(get(task, "status"))
        ))
        .text
}

/// A fixed line of the human view (`comment added`, `progress recorded`).
#[must_use]
pub fn note_line(console: &Console, text: &str) -> String {
    console.print(text).text
}

/// The human view of `task depend`.
#[must_use]
pub fn depend_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} depends on {} task(s); status {}",
            s(get(task, "key")),
            items(get(task, "depends_on")).len(),
            s(get(task, "status"))
        ))
        .text
}

/// The human view of `task relate`.
#[must_use]
pub fn related_line(console: &Console, relation: &Value) -> String {
    console
        .print(&format!("related ({})", s(get(relation, "relation_type"))))
        .text
}

/// The human view of `task link-repo`.
#[must_use]
pub fn linked_line(console: &Console, task: &Value) -> String {
    console
        .print(&format!(
            "{} touches {} repositories",
            s(get(task, "key")),
            items(get(task, "repository_ids")).len()
        ))
        .text
}

/// The human view of `task history`.
#[must_use]
pub fn history_view(console: &Console, events: &Value) -> String {
    let rows: Vec<Vec<String>> = match events {
        Value::List(list) => list
            .iter()
            .map(|event| {
                vec![
                    s(get(event, "seq")),
                    when(get(event, "occurred_at")),
                    s(get(event, "event_type")),
                    s(get(event, "actor")),
                    summary(get(event, "data")),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(&rows, &["seq", "when", "event", "actor", "data"], None)
        .text
}

/// The human view of `task graph`.
#[must_use]
pub fn graph_view(console: &Console, graph: &Value) -> String {
    let nodes = items(get(graph, "nodes"));
    let keys: Vec<(String, String)> = nodes
        .iter()
        .map(|n| (s(get(n, "id")), s(get(n, "key"))))
        .collect();
    let key_of = |id: &str| {
        keys.iter()
            .find(|(candidate, _)| candidate == id)
            .map(|(_, key)| key.clone())
            .unwrap_or_default()
    };
    let node_rows: Vec<Vec<String>> = nodes
        .iter()
        .map(|n| {
            vec![
                s(get(n, "key")),
                s(get(n, "status")),
                if matches!(get(n, "blocked"), Some(Value::Bool(true))) {
                    "BLOCKED".to_owned()
                } else {
                    String::new()
                },
                prefix(&s(get(n, "title")), 60),
            ]
        })
        .collect();
    let edge_rows: Vec<Vec<String>> = items(get(graph, "edges"))
        .iter()
        .map(|e| {
            let kind = s(get(e, "kind"));
            vec![
                key_of(&s(get(e, "source"))),
                if kind == "depends_on" {
                    "blocks".to_owned()
                } else {
                    kind
                },
                key_of(&s(get(e, "target"))),
            ]
        })
        .collect();
    let mut out = console
        .table(&node_rows, &["key", "status", "", "title"], None)
        .text;
    out.push_str(
        &console
            .table(&edge_rows, &["from", "edge", "to"], None)
            .text,
    );
    out
}

/// `_render_verdict`: the `task contract-check` verdict as a key/value block.
#[must_use]
pub fn contract_view(console: &Console, out: &Value) -> String {
    let probability = match get(out, "prediction").filter(|p| !matches!(p, Value::Null)) {
        None => "none (no prediction)".to_owned(),
        Some(prediction) => {
            let interval = match (get(prediction, "low"), get(prediction, "high")) {
                (Some(Value::Null) | None, _) | (_, Some(Value::Null) | None) => "none".to_owned(),
                (low, high) => format!("95% {:.3} to {:.3}", float_of(low), float_of(high)),
            };
            let cutoff = opt(get(prediction, "data_cutoff"))
                .filter(|cutoff| !cutoff.is_empty())
                .unwrap_or_else(|| "none".to_owned());
            format!(
                "{:.3}, interval {interval}, fit {}, data cutoff {cutoff} (inference)",
                float_of(get(prediction, "probability")),
                s(get(prediction, "fit_id"))
            )
        }
    };
    let reasons = texts(get(out, "reasons"), "; ");
    let missing = texts(get(out, "missing"), ", ");
    let target = format!("{:.3}", float_of(get(out, "target")));
    let verdict = s(get(out, "verdict"));
    let model = s(get(out, "model"));
    let pairs: [(&str, Option<&str>); 6] = [
        ("verdict", Some(verdict.as_str())),
        ("reasons", Some(reasons.as_str())),
        ("missing", Some(missing.as_str())),
        ("probability", Some(probability.as_str())),
        ("target", Some(target.as_str())),
        ("model", Some(model.as_str())),
    ];
    console.kv(&pairs, Some("contract check")).text
}

/// A JSON number as a float (`0.0` for anything else).
fn float_of(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Float(number)) => *number,
        Some(Value::Int(number)) => *number as f64,
        _ => 0.0,
    }
}

/// The strings of a list joined with `separator`, or `none` when the list is empty.
fn texts(value: Option<&Value>, separator: &str) -> String {
    let parts: Vec<String> = items(value).iter().map(|item| s(Some(item))).collect();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(separator)
    }
}

/// The human view of `task promote-ready`.
#[must_use]
pub fn promote_view(console: &Console, report: &Value, dry_run: bool) -> String {
    let keys: Vec<String> = items(get(report, "promoted")).iter().map(python).collect();
    let mut line = format!(
        "{} {}",
        if dry_run { "would promote" } else { "promoted" },
        keys.len()
    );
    if !keys.is_empty() {
        line.push_str(&format!(": {}", keys.join(", ")));
    }
    let mut out = console.print(&line).text;
    if let Some(Value::Dict(counts)) = get(report, "by_project") {
        for (slug, number) in counts {
            out.push_str(
                &console
                    .print(&format!(
                        "  {}: {}",
                        escape(&python(&Value::Str(slug.clone()))),
                        python(number)
                    ))
                    .text,
            );
        }
    }
    out
}

const HEAD: [&str; 7] = [
    "key",
    "status",
    "priority",
    "category",
    "title",
    "tags",
    "claimed by",
];

fn task_row(task: &Value) -> Vec<String> {
    let tags: Vec<String> = items(get(task, "tags")).iter().map(python).collect();
    vec![
        s(get(task, "key")),
        s(get(task, "status")),
        s(get(task, "priority")),
        opt(get(task, "category")).unwrap_or_default(),
        prefix(&s(get(task, "title")), 70),
        tags.join(","),
        opt(get(task, "claimed_by")).unwrap_or_default(),
    ]
}

fn summary(data: Option<&Value>) -> String {
    let Some(data) = data else {
        return String::new();
    };
    if let (Some(from), Some(to)) = (get(data, "from"), get(data, "to")) {
        let reason = opt(get(data, "reason")).filter(|r| !r.is_empty());
        return format!(
            "{} -> {}{}",
            python(from),
            python(to),
            reason.map_or_else(String::new, |r| format!(" ({r})"))
        );
    }
    if let Some(Value::Dict(changes)) = get(data, "changes") {
        let names: Vec<String> = changes
            .iter()
            .map(|(k, _)| python(&Value::Str(k.clone())))
            .collect();
        return names.join(", ");
    }
    if let Value::Dict(entries) = data {
        let joined: Vec<String> = entries
            .iter()
            .take(3)
            .map(|(k, v)| format!("{}={}", python(&Value::Str(k.clone())), python(v)))
            .collect();
        return prefix(&joined.join(", "), 80);
    }
    String::new()
}
