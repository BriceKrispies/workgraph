//! The human views of the `automation` commands (`cli/commands/automation.py`): the lines and
//! tables a result prints, as functions of the result document and the console. The arguments are
//! in [`super::args`].

use wg_iface::cli::args::{cell, get, items, opt, python, s, when};
use wg_json::Value;
use wg_render::{Console, escape};
use wg_types::pyval::prefix;

/// Python's `str()` of a datetime value: `2026-10-26 10:07:27.331851+00:00`.
fn datetime_text(value: &Value) -> Option<String> {
    match value {
        Value::DateTime(at) => Some(at.format_isoformat().replacen('T', " ", 1)),
        other => opt(Some(other)),
    }
}

/// `"created rule {name}"`.
#[must_use]
pub fn created_line(console: &Console, rule: &Value) -> String {
    console
        .print(&format!("created rule {}", escape(&s(get(rule, "name")))))
        .text
}

/// `"updated rule {name} (v{version})"`.
#[must_use]
pub fn updated_line(console: &Console, rule: &Value) -> String {
    console
        .print(&format!(
            "updated rule {} (v{})",
            escape(&s(get(rule, "name"))),
            s(get(rule, "version"))
        ))
        .text
}

/// `"{name} enabled"` or `"{name} disabled"`.
#[must_use]
pub fn toggled_line(console: &Console, rule: &Value, enabled: bool) -> String {
    let state = if enabled { "enabled" } else { "disabled" };
    console
        .print(&format!("{} {state}", escape(&s(get(rule, "name")))))
        .text
}

/// The human view of `list`: one row per rule.
#[must_use]
pub fn list_view(console: &Console, rules: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(rules))
        .iter()
        .map(|rule| {
            let trigger = get(rule, "trigger");
            let events = trigger
                .and_then(|t| get(t, "event_types"))
                .map(|types| {
                    items(Some(types))
                        .iter()
                        .map(python)
                        .collect::<Vec<String>>()
                        .join(",")
                })
                .unwrap_or_default();
            let schedule = if events.is_empty() {
                trigger
                    .and_then(|t| opt(get(t, "cron")))
                    .unwrap_or_default()
            } else {
                events
            };
            vec![
                s(get(rule, "name")),
                if matches!(get(rule, "enabled"), Some(Value::Bool(true))) {
                    "on".to_owned()
                } else {
                    "off".to_owned()
                },
                trigger.map(|t| cell(get(t, "type"))).unwrap_or_default(),
                schedule,
                items(get(rule, "actions")).len().to_string(),
                s(get(rule, "created_by")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "rule",
                "enabled",
                "trigger",
                "events/schedule",
                "actions",
                "author",
            ],
            None,
        )
        .text
}

/// The human view of `show`: every field of the rule, the empty ones left out, under its name.
#[must_use]
pub fn show_view(console: &Console, rule: &Value) -> String {
    let fields: Vec<(String, Option<String>)> = match rule {
        Value::Model { fields, .. } => fields
            .iter()
            .map(|(key, value)| (key.to_string(), datetime_text(value)))
            .collect(),
        _ => Vec::new(),
    };
    let shown: Vec<(&str, Option<&str>)> = fields
        .iter()
        .filter(|(_, value)| {
            value
                .as_deref()
                .is_none_or(|text| text != "[]" && text != "{}")
        })
        .map(|(key, value)| (key.as_str(), value.as_deref()))
        .collect();
    let name = s(get(rule, "name"));
    console.kv(&shown, Some(name.as_str())).text
}

/// The human view of `runs`: one row per run, newest first.
#[must_use]
pub fn runs_view(console: &Console, runs: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(runs))
        .iter()
        .map(|run| {
            vec![
                prefix(&s(get(run, "id")), 8),
                prefix(&s(get(run, "rule_id")), 8),
                s(get(run, "trigger_type")),
                s(get(run, "status")),
                s(get(run, "depth")),
                prefix(&s(get(run, "reason")), 50),
                when(get(run, "created_at")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "run", "rule", "trigger", "status", "depth", "reason", "when",
            ],
            None,
        )
        .text
}

/// The human view of `notifications`.
#[must_use]
pub fn notifications_view(console: &Console, rows: &Value) -> String {
    let table: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|note| {
            vec![
                s(get(note, "level")),
                s(get(note, "title")),
                prefix(&s(get(note, "created_at")), 16),
            ]
        })
        .collect();
    console
        .table(&table, &["level", "title", "when"], None)
        .text
}

/// `run {id[:8]}: {status} ({reason})`, the line `trigger` prints.
#[must_use]
pub fn trigger_line(console: &Console, run: &Value) -> String {
    console
        .print(&format!(
            "run {}: {} ({})",
            prefix(&s(get(run, "id")), 8),
            s(get(run, "status")),
            escape(&s(get(run, "reason")))
        ))
        .text
}

/// The human view of `preview`: whether it matched, the condition trace, and what would happen.
#[must_use]
pub fn preview_view(console: &Console, run: &Value) -> String {
    let mut out = console
        .print(&format!(
            "matched: {} ({})",
            python(get(run, "matched").unwrap_or(&Value::Null)),
            escape(&s(get(run, "reason")))
        ))
        .text;
    let rows: Vec<Vec<String>> = items(get(run, "condition_trace"))
        .iter()
        .map(|entry| {
            vec![
                cell(get(entry, "field")),
                cell(get(entry, "op")),
                cell(get(entry, "expected")),
                cell(get(entry, "actual")),
                cell(get(entry, "result")),
            ]
        })
        .collect();
    out.push_str(
        &console
            .table(
                &rows,
                &["field", "op", "expected", "actual", "result"],
                Some("conditions"),
            )
            .text,
    );
    for action in items(get(run, "action_results")) {
        out.push_str(
            &console
                .print(&format!(
                    "  would {}",
                    escape(&cell(get(action, "planned")))
                ))
                .text,
        );
    }
    out
}

/// The `key: value` lines of a dict the way `kv(r.items())` prints them (empty members left out).
fn pairs_view(console: &Console, value: &Value, title: Option<&str>) -> String {
    let fields: Vec<(String, Option<String>)> = match value {
        Value::Dict(entries) => entries
            .iter()
            .map(|(key, member)| (python(&Value::Str(key.clone())), datetime_text(member)))
            .collect(),
        Value::Model { fields, .. } => fields
            .iter()
            .map(|(key, member)| (key.to_string(), datetime_text(member)))
            .collect(),
        _ => Vec::new(),
    };
    let shown: Vec<(&str, Option<&str>)> = fields
        .iter()
        .filter(|(_, text)| text.as_deref().is_none_or(|t| t != "[]" && t != "{}"))
        .map(|(key, text)| (key.as_str(), text.as_deref()))
        .collect();
    console.kv(&shown, title).text
}

/// The human view of `catalog`.
#[must_use]
pub fn catalog_view(console: &Console, catalog: &Value) -> String {
    pairs_view(console, catalog, None)
}

/// The human view of `process`.
#[must_use]
pub fn process_view(console: &Console, result: &Value) -> String {
    pairs_view(console, result, None)
}
