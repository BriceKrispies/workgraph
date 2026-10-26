//! The pure half of the `decision` commands (`cli/commands/decisions.py`): the requests the
//! options make and the human views of the results.
//!
//! The handlers in `wg-cli` read nothing but the matched options; everything that decides what is
//! asked, or how a result looks, is here as a function of its inputs.

use clap::ArgMatches;
use serde_json::{Value as Json, json};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_render::{Console, escape};

use crate::task::{get, items, many, number, one, opt, s, truncate, when};
use crate::tree::{SNAPSHOT_JSON, placeholder};
use crate::{CliFailure, Snapshot, Usage};

/// The usage of `decision list` and `decision request`, as Click prints them in a usage error.
const LIST: Usage = Usage {
    usage: "workgraph decision list [OPTIONS]",
    command: "workgraph decision list",
};
const REQUEST: Usage = Usage {
    usage: "workgraph decision request [OPTIONS] {task}",
    command: "workgraph decision request",
};

/// The statuses `decision list --status` takes.
const STATUSES: [&str; 4] = ["open", "decided", "withdrawn", "all"];

/// The 32 characters of a full decision id; shorter ids are prefixes.
const FULL_ID: usize = 32;

/// How many decisions the prefix lookup lists (`decisions.list(..., "all", 500)`).
pub const PREFIX_LOOKUP_LIMIT: i64 = 500;

/// The clap command of the `decision` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "decision"),
        Err(_) => crate::tree::command("decision"),
    }
}

/// The `--status` and `--limit` of `decision list`.
pub fn list_args(m: &ArgMatches) -> Result<(String, i64), CliFailure> {
    let status = one(m, "--status").unwrap_or_else(|| "open".to_owned());
    if !STATUSES.contains(&status.as_str()) {
        return Err(CliFailure::fail(
            "--status must be open, decided, withdrawn or all",
            2,
        ));
    }
    Ok((status, number(m, "--limit", 50, LIST)?))
}

/// `_parse_option`: `label` or `label::detail`, as the request option it makes.
fn parse_option(spec: &str, recommended: bool) -> Json {
    let (label, detail) = spec.split_once("::").unwrap_or((spec, ""));
    json!({"label": label.trim(), "detail": detail.trim(), "recommended": recommended})
}

/// The task, question and options of `decision request`.
pub fn request_args(m: &ArgMatches) -> Result<(String, String, Vec<Json>), CliFailure> {
    let specs = many(m, "--option");
    let recommend = one(m, "--recommend")
        .map(|_| number(m, "--recommend", 0, REQUEST))
        .transpose()?;
    let count = i64::try_from(specs.len()).unwrap_or(i64::MAX);
    if recommend.is_some_and(|position| !(1..=count).contains(&position)) {
        return Err(CliFailure::fail(
            format!("--recommend must be between 1 and {count}"),
            2,
        ));
    }
    let options = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let position = i64::try_from(index + 1).unwrap_or(i64::MAX);
            parse_option(spec, recommend == Some(position))
        })
        .collect();
    Ok((
        one(m, "task").unwrap_or_default(),
        one(m, "--question").unwrap_or_default(),
        options,
    ))
}

/// The option id, free text and note of `decision choose`.
#[must_use]
pub fn choose_args(m: &ArgMatches) -> [Option<String>; 3] {
    [one(m, "--option"), one(m, "--other"), one(m, "--note")]
}

/// The decision id argument of `show`, `choose` and `withdraw`.
#[must_use]
pub fn decision_ref(m: &ArgMatches) -> String {
    one(m, "decision_id").unwrap_or_default()
}

/// The `--reason` of `decision withdraw`.
#[must_use]
pub fn reason(m: &ArgMatches) -> Option<String> {
    one(m, "--reason")
}

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
                    truncate(&s(get(d, "id")), 8),
                    s(get(d, "task_key")),
                    get(d, "task")
                        .map(|task| s(get(task, "priority")))
                        .unwrap_or_default(),
                    s(get(d, "status")),
                    truncate(&s(get(d, "question")), 70),
                    truncate(&answer(d), 30),
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
    use wg_wire::convention::{dict, text};

    fn row(id: &str) -> Value {
        dict(vec![("id", text(id))])
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

    #[test]
    fn options_split_the_label_from_the_detail() {
        assert_eq!(
            parse_option(" SQLite :: embedded ", true),
            json!({"label": "SQLite", "detail": "embedded", "recommended": true})
        );
        assert_eq!(
            parse_option("Plain", false),
            json!({"label": "Plain", "detail": "", "recommended": false})
        );
    }
}
