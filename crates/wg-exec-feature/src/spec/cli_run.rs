//! Pure: the clap tree of `workgraph run` and the documents and human output of its `get` and
//! `watch` commands (`RunOut` of `orchestration/runs.py`).

use clap::Command;
use wg_clispec::args::{get, items, python, truncate};
use wg_clispec::tree::snapshot_command;
use wg_json::Value;
use wg_model::entity::{AgentAttempt, Job};
use wg_render::Console;
use wg_storecodec::EventRecord;
use wg_types::AttemptStatus;
use wg_wire::convention::{dict, id, text};
use wg_wire::dto::{attempt, event};

/// The `run` command, built from the snapshot so its help and parameters match.
pub fn command() -> Command {
    snapshot_command("run")
}

/// `RunOut`: the attempt, the id and status of its `agent_run` job (absent when it has none) and
/// the recent events, oldest first. `events` are the rows as the query returned them, newest first;
/// each is written as `Event.to_dict()`.
pub fn run_document(attempt: &AgentAttempt, job: Option<&Job>, events: &[EventRecord]) -> Value {
    let recent: Vec<Value> = events
        .iter()
        .rev()
        .map(|record| event::dict_from_stored(&record.clone().into()))
        .collect();
    dict(vec![
        ("attempt", attempt::from_model(attempt)),
        ("job_id", job.map_or(Value::Null, |found| id(found.id))),
        (
            "job_status",
            job.map_or(Value::Null, |found| text(found.status.as_str())),
        ),
        ("recent_events", Value::List(recent)),
    ])
}

/// The first eight characters of an id (`short`).
fn short(value: Option<&Value>) -> String {
    python(value.unwrap_or(&Value::Null))
        .chars()
        .take(8)
        .collect()
}

/// A table cell: a value as text, `None` (and absence) as nothing.
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(value) => python(value),
    }
}

/// The optional text of a `kv` pair: `None` for an absent or null value (`kv` leaves it out).
fn optional(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(value) => Some(python(value)),
    }
}

/// `run get`: the attempt's `kv` block, then its recent events as a table.
#[must_use]
pub fn get_view(console: &Console, run: &Value) -> String {
    let attempt = get(run, "attempt");
    let title = format!(
        "attempt {}",
        short(attempt.and_then(|found| get(found, "id")))
    );
    let status = attempt.and_then(|found| optional(get(found, "status")));
    let job_status = optional(get(run, "job_status"));
    let exit = attempt.and_then(|found| optional(get(found, "exit_reason")));
    let error = attempt.and_then(|found| optional(get(found, "error")));
    let pairs = [
        ("status", status.as_deref()),
        ("job", job_status.as_deref()),
        ("exit", exit.as_deref()),
        ("error", error.as_deref()),
    ];
    let header = console.kv(&pairs, Some(&title)).text;
    let rows: Vec<Vec<String>> = items(get(run, "recent_events"))
        .iter()
        .map(|event| {
            let message = get(event, "data")
                .and_then(|data| get(data, "message"))
                .map(python)
                .unwrap_or_default();
            vec![
                cell(get(event, "seq")),
                cell(get(event, "event_type")),
                truncate(&message, 80),
            ]
        })
        .collect();
    format!(
        "{header}{}",
        console
            .table(&rows, &["seq", "event", "message"], None)
            .text
    )
}

/// The sequence number of a progress event as an integer (`0` when the document has none).
#[must_use]
pub fn seq_of(event: &Value) -> i64 {
    match get(event, "seq") {
        Some(Value::Int(seq)) => i64::try_from(*seq).unwrap_or(0),
        _ => 0,
    }
}

/// One progress line of `run watch`, as markup: `[dim]<event type>[/dim] <message>`.
#[must_use]
pub fn progress_markup(event: &Value) -> String {
    let message = get(event, "data")
        .and_then(|data| get(data, "message"))
        .map(python)
        .unwrap_or_default();
    format!("[dim]{}[/dim] {message}", cell(get(event, "event_type")))
}

/// Whether the attempt of a `RunOut` document has finished (`TERMINAL_ATTEMPT_STATUSES`).
#[must_use]
pub fn finished(run: &Value) -> bool {
    get(run, "attempt")
        .and_then(|attempt| get(attempt, "status"))
        .is_some_and(|status| {
            AttemptStatus::parse(&python(status)).is_some_and(wg_exec::attempt::is_terminal)
        })
}
