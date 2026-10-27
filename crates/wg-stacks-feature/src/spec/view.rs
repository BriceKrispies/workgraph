//! The human views of the `stack` commands (`cli/commands/stacks.py`): the key-value block, tables
//! and notices a result prints, as functions of the result document and the console. Each returns
//! the text Python's `console` would write, line by line.

use serde_json::Value as Json;
use wg_clispec::args::{cell, get, items, opt, python, s, when};
use wg_json::Value;
use wg_render::Console;
use wg_types::pyval::prefix;

/// `short(value, n)`: the first `n` characters, or nothing when there is no value.
fn short(value: Option<&Value>, n: usize) -> String {
    prefix(&cell(value), n)
}

/// A JSON scalar as Python's `str()` shows it (`None` for null, `True`/`False` for booleans).
fn json_text(value: &Json) -> String {
    match value {
        Json::Null => "None".to_owned(),
        Json::Bool(true) => "True".to_owned(),
        Json::Bool(false) => "False".to_owned(),
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The strings of a JSON list, or none (`x or []`).
fn json_strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `_detail_summary`: one line for an item's detail, naming its conflicting paths and its ejection
/// when it has either. A detail that is not a JSON object is shown as it is.
#[must_use]
pub fn detail_summary(text: Option<&str>) -> String {
    let text = text.unwrap_or_default();
    if text.is_empty() {
        return String::new();
    }
    let parsed: Json = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(_) => return text.to_owned(),
    };
    let Some(detail) = parsed.as_object() else {
        return text.to_owned();
    };
    let conflicts = detail
        .get("conflicts")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
        .map(|(repository, conflict)| {
            let paths = json_strings(conflict.get("paths")).join(", ");
            let paths = if paths.is_empty() {
                "no paths reported".to_owned()
            } else {
                paths
            };
            format!("conflict in {repository}: {paths}")
        });
    let ejection = detail
        .get("ejection")
        .and_then(Json::as_object)
        .filter(|ejection| !ejection.is_empty())
        .map(|ejection| {
            let failed = json_strings(ejection.get("failed_checks")).join(", ");
            let position = ejection
                .get("position")
                .map_or_else(|| "None".to_owned(), json_text);
            format!("ejected at position {position}: {failed}")
        });
    conflicts
        .chain(ejection)
        .collect::<Vec<String>>()
        .join("; ")
}

/// `_render_stack`: the stack's key-value block, its repositories, its items, and the notice of a
/// verified stack.
#[must_use]
pub fn stack_view(console: &Console, stack: &Value) -> String {
    let parent_key = s(get(stack, "parent_key"));
    let title = format!("stack {parent_key}");
    let status = s(get(stack, "status"));
    let id = s(get(stack, "id"));
    let parent_title = s(get(stack, "parent_title"));
    let key_block = console
        .kv(
            &[
                ("parent", Some(parent_key.as_str())),
                ("title", Some(parent_title.as_str())),
                ("status", Some(status.as_str())),
                ("id", Some(id.as_str())),
            ],
            Some(&title),
        )
        .text;
    let repositories: Vec<Vec<String>> = items(get(stack, "repositories"))
        .iter()
        .map(|row| {
            vec![
                s(get(row, "repository_name")),
                short(get(row, "base_sha"), 12),
                s(get(row, "integration_branch")),
                short(get(row, "integration_tip"), 12),
            ]
        })
        .collect();
    let repository_table = console
        .table(
            &repositories,
            &["repository", "base", "integration branch", "tip"],
            None,
        )
        .text;
    let stack_items = items(get(stack, "items"));
    let rows: Vec<Vec<String>> = stack_items
        .iter()
        .map(|item| {
            let merge_order = match get(item, "merge_order") {
                None | Some(Value::Null | Value::Int(0)) => String::new(),
                Some(order) => python(order),
            };
            vec![
                merge_order,
                s(get(item, "task_key")),
                s(get(item, "state")),
                detail_summary(opt(get(item, "detail")).as_deref()),
                s(get(item, "task_title")),
            ]
        })
        .collect();
    let item_table = console
        .table(
            &rows,
            &["merge order", "task", "state", "detail", "title"],
            None,
        )
        .text;
    let unfinished: Vec<String> = stack_items
        .iter()
        .filter(|item| s(get(item, "state")) != "verified")
        .map(|item| format!("{} ({})", s(get(item, "task_key")), s(get(item, "state"))))
        .collect();
    let notice = match (status == "verified", unfinished.is_empty()) {
        (false, _) => String::new(),
        (true, true) => console
            .print(&format!(
                "Verified on the integration tip. Land it with: workgraph task accept {parent_key}"
            ))
            .text,
        (true, false) => console
            .print(&format!(
                "Verified on the integration tip. Landing releases the unverified children from the stack ({}) and lands the rest. Land it with: workgraph task accept {parent_key}",
                unfinished.join(", ")
            ))
            .text,
    };
    [key_block, repository_table, item_table, notice].concat()
}

/// `_event_summary`: the event's fields that carry a value, as `key=value`.
fn event_summary(event: &Value) -> String {
    const KEYS: [&str; 8] = [
        "task_key",
        "from",
        "to",
        "position",
        "passed",
        "run_id",
        "verification_run_id",
        "failed_checks",
    ];
    let data = get(event, "data");
    KEYS.iter()
        .filter_map(|key| {
            let value = data.and_then(|data| get(data, key))?;
            let empty = match value {
                Value::Null => true,
                Value::List(members) => members.is_empty(),
                Value::Dict(entries) => entries.is_empty(),
                other => python(other).is_empty(),
            };
            (!empty).then(|| format!("{key}={}", python(value)))
        })
        .collect::<Vec<String>>()
        .join(", ")
}

/// `_render_history`: the stack's events in order.
#[must_use]
pub fn history_view(console: &Console, events: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(events))
        .iter()
        .map(|event| {
            vec![
                s(get(event, "seq")),
                when(get(event, "occurred_at")),
                s(get(event, "event_type")),
                event_summary(event),
            ]
        })
        .collect();
    console
        .table(&rows, &["seq", "time", "event", "detail"], None)
        .text
}

/// `_render_integration`: one row per repository's integration job.
#[must_use]
pub fn integration_view(console: &Console, out: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(out, "jobs"))
        .iter()
        .map(|job| vec![s(get(job, "repository")), s(get(job, "job_id"))])
        .collect();
    console
        .table(&rows, &["repository", "integration job"], None)
        .text
}

/// `_render_verification`: the stack and its verification job.
#[must_use]
pub fn verification_view(console: &Console, out: &Value) -> String {
    let rows = vec![vec![s(get(out, "stack")), s(get(out, "job_id"))]];
    console
        .table(&rows, &["stack", "verification job"], None)
        .text
}

/// The `stack list` table: one row per stack.
#[must_use]
pub fn list_view(console: &Console, stacks: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(stacks))
        .iter()
        .map(|stack| {
            vec![
                s(get(stack, "parent_key")),
                s(get(stack, "status")),
                items(get(stack, "repositories")).len().to_string(),
                items(get(stack, "items")).len().to_string(),
                s(get(stack, "parent_title")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &["parent", "status", "repositories", "tasks", "title"],
            None,
        )
        .text
}
