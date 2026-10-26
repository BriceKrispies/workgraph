//! Pure: the human views of the `claude` commands (`common.kv`, `common.table`, the console lines
//! and the `print_json` of `mcp-config`). Each returns the text Python writes; the handlers in
//! [`crate::cli`] send it to the output stream.

use serde_json::{Map, Value};
use wg_clispec::args::python;
use wg_render::Console;
use wg_wire::convention::json_value;

/// `common.kv(d.items(), title)`: a value Python leaves out (`None`, an empty list, an empty dict)
/// is skipped; every other value is `str()` of it. A value that is not an object prints the title
/// only.
#[must_use]
pub fn kv_object(console: &Console, value: &Value, title: &str) -> String {
    let empty = Map::new();
    let object = value.as_object().unwrap_or(&empty);
    let shown: Vec<(&str, Option<String>)> = object
        .iter()
        .map(|(key, value)| (key.as_str(), shown_value(value)))
        .collect();
    let pairs: Vec<(&str, Option<&str>)> = shown
        .iter()
        .map(|(key, value)| (*key, value.as_deref()))
        .collect();
    console.kv(&pairs, Some(title)).text
}

/// `str(value)` of a JSON value, or `None` when `kv` leaves it out.
fn shown_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Array(items) if items.is_empty() => None,
        Value::Object(members) if members.is_empty() => None,
        other => Some(python(&json_value(other))),
    }
}

/// The `status` view: the installation, the ingestion counts and the sessions table.
#[must_use]
pub fn status_view(console: &Console, data: &Value) -> String {
    let none: &[Value] = &[];
    let sessions = data
        .get("sessions")
        .and_then(Value::as_array)
        .map_or(none, Vec::as_slice);
    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|session| {
            vec![
                prefix(&field(session, "external_session_id"), 12),
                field(session, "binding"),
                prefix(&field(session, "task_id"), 8),
                field(session, "tool_calls"),
                field(session, "tool_failures"),
                field(session, "started_at"),
            ]
        })
        .collect();
    let table = console.table(
        &rows,
        &["session", "binding", "task", "tools", "failed", "started"],
        None,
    );
    format!(
        "{}{}{}",
        kv_object(
            console,
            data.get("installation").unwrap_or(&Value::Null),
            "installation"
        ),
        kv_object(
            console,
            data.get("ingestion").unwrap_or(&Value::Null),
            "ingestion"
        ),
        table.text
    )
}

/// The `sessions` table: the full external id, the binding, the task and attempt ids and the
/// tool count.
#[must_use]
pub fn sessions_view(console: &Console, sessions: &[Value]) -> String {
    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|session| {
            vec![
                field(session, "external_session_id"),
                field(session, "binding"),
                field(session, "task_id"),
                field(session, "attempt_id"),
                field(session, "tool_calls"),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &["session", "binding", "task", "attempt", "tools"],
            None,
        )
        .text
}

/// `bound {session_id or cwd} to {task}`, with Python's `None` when neither is set.
#[must_use]
pub fn bind_line(console: &Console, result: &Value) -> String {
    let who = [field(result, "session_id"), field(result, "cwd")]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| "None".to_owned());
    console
        .print(&format!("bound {who} to {}", field(result, "task")))
        .text
}

/// `deactivated {count} binding(s)`.
#[must_use]
pub fn unbind_line(console: &Console, count: usize) -> String {
    console
        .print(&format!("deactivated {count} binding(s)"))
        .text
}

/// The `mcp-config` block as `console.print_json(json.dumps(r))`: indented JSON, coloured on a
/// terminal, with the trailing newline `print` adds.
#[must_use]
pub fn mcp_config_view(console: &Console, config: &Value) -> String {
    let mut text = serde_json::to_string_pretty(config).unwrap_or_default();
    text.push('\n');
    console.print_json(&text)
}

/// A cell's text: `str(value)`, empty for `None`.
fn field(object: &Value, key: &str) -> String {
    match object.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => python(&json_value(&Value::Bool(*flag))),
        Some(other) => other.to_string(),
    }
}

/// `text[:n]`, counted in characters.
fn prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kv_skips_none_and_empty_containers_only() {
        let console = Console::new(80);
        let object = json!({"a": null, "b": [], "c": {}, "d": false, "e": ["x"], "f": 0});
        assert_eq!(
            kv_object(&console, &object, "t"),
            "t\n  d: False\n  e: ['x']\n  f: 0\n"
        );
    }

    #[test]
    fn bind_line_falls_back_to_the_cwd() {
        let console = Console::new(80);
        let result = json!({"session_id": null, "cwd": "/w", "task": "WG-1"});
        assert_eq!(bind_line(&console, &result), "bound /w to WG-1\n");
        assert_eq!(unbind_line(&console, 2), "deactivated 2 binding(s)\n");
    }
}
