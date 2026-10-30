//! The human views of the `decision` commands and the prefix lookup (`cli/commands/decisions.py`):
//! functions of the values the services return, with no clap and no I/O.

use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{get, items, opt, s, when};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_render::{Console, escape};
use wg_types::pyval::prefix;

/// The 32 characters of a full decision id; shorter ids are prefixes.
const FULL_ID: usize = 32;

/// How many decisions the prefix lookup lists (`decisions.list(..., "all", 500)`).
pub const PREFIX_LOOKUP_LIMIT: i64 = 500;

/// Whether a reference is already a full id (`_full_id` returns it as it is).
#[must_use]
pub fn is_full_id(reference: &str) -> bool {
    reference.len() >= FULL_ID
}

/// `_full_id`: the one decision id of `listed` (the `decision list --status all` rows) that starts
/// with `prefix`. Several matches are an error with code 2, none an error with code 4.
pub fn expand_prefix(prefix: &str, listed: &Value) -> Result<String, CliFailure> {
    let rows = match listed {
        Value::List(rows) => rows.as_slice(),
        _ => &[],
    };
    let matches: Vec<String> = rows
        .iter()
        .map(|row| s(get(row, "id")))
        .filter(|id| id.starts_with(prefix))
        .collect();
    match matches.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(CliFailure::fail(
            format!("decision {} not found", py_repr_str(prefix)),
            4,
        )),
        _ => Err(CliFailure::fail(
            format!("decision {} is ambiguous", py_repr_str(prefix)),
            2,
        )),
    }
}

/// `_answer`: the free text (as `other: ...`), else the chosen option id, else nothing.
fn answer(decision: &Value) -> String {
    match opt(get(decision, "other_text")) {
        Some(text) => format!("other: {text}"),
        None => opt(get(decision, "chosen_option_id")).unwrap_or_default(),
    }
}

/// The human view of `decision list`.
#[must_use]
pub fn list_view(console: &Console, decisions: &Value) -> String {
    let rows: Vec<Vec<String>> = match decisions {
        Value::List(list) => list
            .iter()
            .map(|d| {
                vec![
                    prefix(&s(get(d, "id")), 8),
                    s(get(d, "task_key")),
                    get(d, "task")
                        .map(|task| s(get(task, "priority")))
                        .unwrap_or_default(),
                    s(get(d, "status")),
                    prefix(&s(get(d, "question")), 70),
                    prefix(&answer(d), 30),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(
            &rows,
            &["id", "task", "priority", "status", "question", "answer"],
            None,
        )
        .text
}

/// `_render_decision`.
#[must_use]
pub fn decision_view(console: &Console, d: &Value) -> String {
    let said = answer(d);
    let decided = opt(get(d, "decided_at")).map(|_| when(get(d, "decided_at")));
    let created = when(get(d, "created_at"));
    let task_key = s(get(d, "task_key"));
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("id", opt(get(d, "id"))),
        ("task", Some(task_key.clone())),
        ("status", opt(get(d, "status"))),
        ("answer", (!said.is_empty()).then_some(said)),
        ("note", opt(get(d, "note"))),
        ("decided by", opt(get(d, "decided_by"))),
        ("decided", decided),
        ("created", Some(created)),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    let mut out = console
        .kv(&borrowed, Some(&format!("decision {task_key}")))
        .text;
    out.push_str(
        &console
            .print(&format!("\n{}", escape(&s(get(d, "question")))))
            .text,
    );
    for option in items(get(d, "options")) {
        let mark = if matches!(get(option, "recommended"), Some(Value::Bool(true))) {
            " (recommended)"
        } else {
            ""
        };
        let detail = opt(get(option, "detail"))
            .filter(|text| !text.is_empty())
            .map(|text| format!(" - {}", escape(&text)))
            .unwrap_or_default();
        out.push_str(
            &console
                .print(&format!(
                    "  [bold]{}[/bold] {}{mark}{detail}",
                    s(get(option, "id")),
                    escape(&s(get(option, "label")))
                ))
                .text,
        );
    }
    out
}

/// The human view of `decision request`.
#[must_use]
pub fn requested_line(console: &Console, d: &Value) -> String {
    console
        .print(&format!(
            "decision {} requested on {}",
            s(get(d, "id")),
            s(get(d, "task_key"))
        ))
        .text
}

/// The human view of `decision choose`.
#[must_use]
pub fn chosen_line(console: &Console, d: &Value) -> String {
    console
        .print(&format!(
            "decision {} on {} decided: {}",
            s(get(d, "id")),
            s(get(d, "task_key")),
            answer(d)
        ))
        .text
}

/// The human view of `decision withdraw`.
#[must_use]
pub fn withdrawn_line(console: &Console, d: &Value) -> String {
    console
        .print(&format!(
            "decision {} on {} withdrawn",
            s(get(d, "id")),
            s(get(d, "task_key"))
        ))
        .text
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_json::dict;
    use wg_wire::convention::text;

    fn row(id: &str) -> Value {
        dict!({ "id": text(id) })
    }

    #[test]
    fn a_prefix_expands_to_the_one_decision_it_starts() {
        let listed = Value::List(vec![row("aaaa1111-x"), row("bbbb2222-y")]);
        assert_eq!(
            expand_prefix("bbbb", &listed).ok().as_deref(),
            Some("bbbb2222-y")
        );
    }

    #[test]
    fn no_match_exits_4_and_several_exit_2() {
        let listed = Value::List(vec![row("aaaa1111-x"), row("aaaa2222-y")]);
        let code = |prefix: &str| expand_prefix(prefix, &listed).err().map(|e| e.exit_code());
        assert_eq!(code("cccc"), Some(4));
        assert_eq!(code("aaaa"), Some(2));
    }
}
