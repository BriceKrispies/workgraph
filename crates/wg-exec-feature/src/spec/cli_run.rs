//! Pure: the clap tree of `workgraph run` and the documents and human output of its `get` and
//! `watch` commands (`RunOut` of `orchestration/runs.py`).

use clap::Command;
use wg_clispec::args::{cell, get, items, python};
use wg_clispec::tree::snapshot_command;
use wg_json::{Value, dict};
use wg_model::entity::{AgentAttempt, Job};
use wg_render::Console;
use wg_rows::EventRecord;
use wg_types::AttemptStatus;
use wg_types::pyval::prefix;
use wg_wire::convention::{id, text};
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
    dict!({
        "attempt": attempt::from_model(attempt),
        "job_id": job.map_or(Value::Null, |found| id(found.id)),
        "job_status": job.map_or(Value::Null, |found| text(found.status.as_str())),
        "recent_events": Value::List(recent),
    })
}

/// The first eight characters of an id (`short`).
fn short(value: Option<&Value>) -> String {
    prefix(&python(value.unwrap_or(&Value::Null)), 8)
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
                prefix(&message, 80),
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

/// `run start`: `attempt <id> #<number> <status>`.
#[must_use]
pub fn start_line(console: &Console, attempt: &Value) -> String {
    console
        .print(&format!(
            "attempt {} #{} {}",
            cell(get(attempt, "id")),
            cell(get(attempt, "number")),
            cell(get(attempt, "status"))
        ))
        .text
}

/// `run cancel`: `<short id>: <status>`.
#[must_use]
pub fn cancel_line(console: &Console, attempt: &Value) -> String {
    console
        .print(&format!(
            "{}: {}",
            short(get(attempt, "id")),
            cell(get(attempt, "status"))
        ))
        .text
}

/// `run retry`: `retry queued as attempt <id> #<number>`.
#[must_use]
pub fn retry_line(console: &Console, attempt: &Value) -> String {
    console
        .print(&format!(
            "retry queued as attempt {} #{}",
            cell(get(attempt, "id")),
            cell(get(attempt, "number"))
        ))
        .text
}

/// `run rerun`: where the rerun was queued, whether the baseline was reproduced and what was missing.
#[must_use]
pub fn rerun_line(console: &Console, result: &Value) -> String {
    let missing = get(result, "missing_revisions");
    let tail = if items(missing).is_empty() {
        String::new()
    } else {
        format!("; missing revisions: {}", cell(missing))
    };
    console
        .print(&format!(
            "rerun queued as {}; baseline reproduced: {}{tail}",
            cell(get(result, "attempt").and_then(|found| get(found, "id"))),
            cell(get(result, "reproduced_baseline"))
        ))
        .text
}

/// `run schedule --explain`: the task, whether it is eligible and the reasons it is not.
#[must_use]
pub fn explain_view(console: &Console, report: &Value) -> String {
    let task = optional(get(report, "task"));
    let eligible = optional(get(report, "eligible"));
    let reasons = optional(get(report, "reasons"));
    console
        .kv(
            &[
                ("task", task.as_deref()),
                ("eligible", eligible.as_deref()),
                ("reasons", reasons.as_deref()),
            ],
            None,
        )
        .text
}

/// `run schedule --dry-run`: the eligible tasks.
#[must_use]
pub fn eligible_view(console: &Console, rows: &Value) -> String {
    let table: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|row| {
            vec![
                cell(get(row, "key")),
                cell(get(row, "priority")),
                cell(get(row, "agent")),
            ]
        })
        .collect();
    console
        .table(&table, &["task", "priority", "agent"], None)
        .text
}

/// `run schedule`: how many runs the pass started, then the whole report.
#[must_use]
pub fn tick_line(console: &Console, report: &Value) -> String {
    console
        .print(&format!(
            "started {}: {}",
            items(get(report, "started")).len(),
            python(report)
        ))
        .text
}

/// `run decompose`: the proposed children, then the keys created (or a dry run note).
#[must_use]
pub fn decompose_view(console: &Console, result: &Value) -> String {
    let children = get(result, "proposal").and_then(|found| get(found, "children"));
    let rows: Vec<Vec<String>> = items(children)
        .iter()
        .map(|child| {
            let depends: Vec<String> = items(get(child, "depends_on")).iter().map(python).collect();
            vec![
                cell(get(child, "ref")),
                cell(get(child, "title")),
                depends.join(","),
            ]
        })
        .collect();
    let created = get(result, "created");
    let note = if items(created).is_empty() {
        "dry run (use --create)".to_owned()
    } else {
        format!("created: {}", cell(created))
    };
    format!(
        "{}{}",
        console
            .table(&rows, &["ref", "title", "depends on"], None)
            .text,
        console.print(&note).text
    )
}
