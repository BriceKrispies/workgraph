//! The pure half of the `worker` and `job` commands (`cli/commands/jobs.py`): the clap command of
//! each group, the option readings and the human views of the results.
//!
//! The handlers in `wg-cli` read nothing but the matched options; the rest is here as a function
//! of its inputs.

use clap::ArgMatches;
use wg_json::{PyStr, Value};
use wg_render::{Console, escape};

use crate::args::{flag, float_value, get, items, many, number, one, python, s, when};
use crate::tree::{SNAPSHOT_JSON, placeholder};
use crate::{CliFailure, Snapshot, Usage};
use wg_types::pyval::prefix;

/// Click's usage line of `job list`.
const JOB_LIST: Usage = Usage {
    usage: "workgraph job list [OPTIONS]",
    command: "workgraph job list",
};

/// Click's usage line of `worker run`.
const WORKER_RUN: Usage = Usage {
    usage: "workgraph worker run [OPTIONS]",
    command: "workgraph worker run",
};

/// The clap command of the `job` group: the snapshot's own tree.
#[must_use]
pub fn job_command() -> clap::Command {
    group_command("job")
}

/// The clap command of the `worker` group: the snapshot's own tree.
#[must_use]
pub fn worker_command() -> clap::Command {
    group_command("worker")
}

fn group_command(name: &str) -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, name),
        Err(_) => crate::tree::command(name),
    }
}

/// `--concurrency`: an optional count of concurrent jobs.
pub fn concurrency(m: &ArgMatches) -> Result<Option<usize>, CliFailure> {
    match one(m, "--concurrency") {
        None => Ok(None),
        Some(_) => {
            let value = number(m, "--concurrency", 0, WORKER_RUN)?;
            usize::try_from(value).map(Some).map_err(|_| {
                CliFailure::fail(
                    format!("Invalid value for '--concurrency': {value} is out of range."),
                    2,
                )
            })
        }
    }
}

/// `--kind` (repeatable) of `worker run`.
pub fn worker_kinds(m: &ArgMatches) -> Vec<String> {
    many(m, "--kind")
}

/// `--once`.
pub fn once(m: &ArgMatches) -> bool {
    flag(m, "--once")
}

/// `--max-idle`, in seconds.
pub fn max_idle(m: &ArgMatches) -> Result<Option<f64>, CliFailure> {
    float_value(m, "--max-idle", WORKER_RUN)
}

/// `--status`/`-s` (repeatable) of `job list`.
pub fn statuses(m: &ArgMatches) -> Vec<String> {
    many(m, "--status")
}

/// `--kind` of `job list`.
pub fn kind_filter(m: &ArgMatches) -> Option<String> {
    one(m, "--kind")
}

/// `--limit` and `--offset` of `job list`.
pub fn page(m: &ArgMatches) -> Result<(i64, i64), CliFailure> {
    Ok((
        number(m, "--limit", 50, JOB_LIST)?,
        number(m, "--offset", 0, JOB_LIST)?,
    ))
}

/// `job_id` of the single-job commands.
pub fn job_id(m: &ArgMatches) -> String {
    one(m, "job_id").unwrap_or_default()
}

/// `job enqueue KIND --payload --key`: the kind, the payload text and the idempotency key.
pub fn enqueue_args(m: &ArgMatches) -> (String, String, Option<String>) {
    (
        one(m, "kind").unwrap_or_default(),
        one(m, "--payload").unwrap_or_else(|| "{}".to_owned()),
        one(m, "--key"),
    )
}

/// `job compact --dry-run/--apply`: a dry run unless `--apply` was given.
pub fn compact_dry_run(m: &ArgMatches) -> bool {
    !flag(m, "--dry-run__secondary")
}

/// Python `kv` rows: a pair whose value is empty (`None`, `[]`, `{}`) is left out.
fn kv_rows(pairs: Vec<(String, Value)>) -> Vec<(String, Option<String>)> {
    pairs
        .into_iter()
        .map(|(key, value)| {
            let empty = match &value {
                Value::Null => true,
                Value::List(members) => members.is_empty(),
                Value::Dict(entries) => entries.is_empty(),
                _ => false,
            };
            let text = (!empty).then(|| python(&value));
            (key, text)
        })
        .collect()
}

fn kv(console: &Console, pairs: Vec<(String, Value)>, title: Option<&str>) -> String {
    let rows = kv_rows(pairs);
    let borrowed: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|(key, text)| (key.as_str(), text.as_deref()))
        .collect();
    console.kv(&borrowed, title).text
}

/// A dict key as Python prints it.
fn key_text(key: &PyStr) -> String {
    python(&Value::Str(key.clone()))
}

/// A cell value as the Python table prints it: an absent value is an empty cell.
fn plain(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(other) => python(other),
    }
}

/// `_job_row`: the list's columns for one job.
fn job_row(job: &Value) -> Vec<String> {
    let attempts = format!(
        "{}/{}",
        s(get(job, "attempts")),
        s(get(job, "max_attempts"))
    );
    vec![
        prefix(&s(get(job, "id")), 8),
        s(get(job, "kind")),
        s(get(job, "status")),
        attempts,
        when(get(job, "run_at")),
        prefix(&plain(get(job, "last_error")), 60),
    ]
}

/// `job list`: the page as a table, or `nothing to show`.
#[must_use]
pub fn job_list_view(console: &Console, page: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(page, "items")).iter().map(job_row).collect();
    console
        .table(
            &rows,
            &["job", "kind", "status", "attempts", "run at", "error"],
            None,
        )
        .text
}

/// `job show`: the job's fields, then its runs as a table.
#[must_use]
pub fn job_show_view(console: &Console, job: &Value) -> String {
    let pairs: Vec<(String, Value)> = match job {
        Value::Dict(entries) => entries
            .iter()
            .filter(|(key, _)| key_text(key) != "runs")
            .map(|(key, value)| (key_text(key), value.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let title = s(get(job, "kind"));
    let mut text = kv(console, pairs, Some(&title));
    let rows: Vec<Vec<String>> = items(get(job, "runs"))
        .iter()
        .map(|run| {
            vec![
                s(get(run, "attempt")),
                s(get(run, "worker")),
                s(get(run, "status")),
                prefix(&plain(get(run, "error")), 80),
            ]
        })
        .collect();
    text.push_str(
        &console
            .table(&rows, &["#", "worker", "status", "error"], None)
            .text,
    );
    text
}

/// `job stats`: the counts under the title `jobs`.
#[must_use]
pub fn stats_view(console: &Console, stats: &Value) -> String {
    let pairs: Vec<(String, Value)> = match stats {
        Value::Dict(entries) => entries
            .iter()
            .map(|(key, value)| (key_text(key), value.clone()))
            .collect(),
        _ => Vec::new(),
    };
    kv(console, pairs, Some("jobs"))
}

/// `job cancel`: `JOB: STATUS`.
#[must_use]
pub fn cancelled_line(console: &Console, out: &Value) -> String {
    console
        .print(&format!(
            "{}: {}",
            escape(&s(get(out, "job_id"))),
            escape(&s(get(out, "status")))
        ))
        .text
}

/// `job retry`: `JOB re-queued`.
#[must_use]
pub fn retried_line(console: &Console, out: &Value) -> String {
    console
        .print(&format!("{} re-queued", escape(&s(get(out, "job_id")))))
        .text
}

/// `job enqueue`: `enqueued JOB`.
#[must_use]
pub fn enqueued_line(console: &Console, out: &Value) -> String {
    console
        .print(&format!("enqueued {}", escape(&s(get(out, "job_id")))))
        .text
}

/// `job compact`: the report under the title `retention`.
#[must_use]
pub fn compact_view(console: &Console, report: &Value) -> String {
    let pairs: Vec<(String, Value)> = match report {
        Value::Dict(entries) => entries
            .iter()
            .map(|(key, value)| (key_text(key), value.clone()))
            .collect(),
        _ => Vec::new(),
    };
    kv(console, pairs, Some("retention"))
}

/// `worker list`: the workers as a table, or `nothing to show`.
#[must_use]
pub fn worker_list_view(console: &Console, workers: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(workers))
        .iter()
        .map(|worker| {
            vec![
                s(get(worker, "id")),
                s(get(worker, "status")),
                s(get(worker, "pid")),
                s(get(worker, "concurrency")),
                items(get(worker, "current")).len().to_string(),
                s(get(worker, "completed")),
                s(get(worker, "failed")),
                when(get(worker, "heartbeat_at")),
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "worker",
                "status",
                "pid",
                "conc",
                "running",
                "done",
                "failed",
                "heartbeat",
            ],
            None,
        )
        .text
}

/// `worker recover`: the report under the title `recovery`.
#[must_use]
pub fn recovery_view(console: &Console, report: &Value) -> String {
    let pairs: Vec<(String, Value)> = match report {
        Value::Dict(entries) => entries
            .iter()
            .map(|(key, value)| (key_text(key), value.clone()))
            .collect(),
        _ => Vec::new(),
    };
    kv(console, pairs, Some("recovery"))
}

/// `worker run`: the line printed when the worker starts.
#[must_use]
pub fn started_line(console: &Console, worker_id: &str, concurrency: usize) -> String {
    console
        .print(&format!(
            "worker {} started (concurrency {concurrency})",
            escape(worker_id)
        ))
        .text
}

/// `worker run`: the line printed when the worker stops.
#[must_use]
pub fn stopped_line(console: &Console, summary: &Value) -> String {
    console
        .print(&format!(
            "worker stopped: {} completed, {} failed",
            escape(&s(get(summary, "completed"))),
            escape(&s(get(summary, "failed")))
        ))
        .text
}
