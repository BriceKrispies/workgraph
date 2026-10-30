//! `workgraph run start, get, watch, cancel, retry, rerun, schedule, decompose` (IF-10, C127 and
//! C132).
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler builds its request, calls one service entry point and prints the result.

use std::time::{Duration, Instant};

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{get, option_flag, option_text, python};
use wg_json::Value;
use wg_runtime::{CliGroup, CliState, human_context};
use wg_wire::convention::json_value;

use crate::service::agents::LiveAgents;
use crate::service::run_flow::{run_cancel, run_rerun, run_retry, run_start};
use crate::service::{runs, scheduler};
use crate::spec::cli_run as view;
use crate::spec::runs::DEFAULT_CANCEL_REASON;
use crate::spec::runs::StartRequest;

/// How many recent events `run get` and `run watch` show.
const RECENT_EVENTS: i64 = 50;

/// How long `run watch` waits for the attempt to finish (`_wait`'s default timeout).
const WATCH_TIMEOUT: Duration = Duration::from_secs(7200);

/// The interval between two reads of `run watch`.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// The `run` group.
pub const GROUP: CliGroup = CliGroup {
    name: "run",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    // The attempt, a positional argument of the commands that take one.
    let attempt = option_text(m, "attempt_id").unwrap_or_default();
    match path.get(1).map(String::as_str) {
        Some("get") => show(state, &attempt),
        Some("watch") => {
            wait(state, &attempt)?;
            show(state, &attempt)
        }
        Some("start") => start(state, m),
        Some("cancel") => {
            let reason =
                option_text(m, "--reason").unwrap_or_else(|| DEFAULT_CANCEL_REASON.to_owned());
            let context = human_context(None)?;
            let out = run_cancel(state.app()?, &context, &attempt, &reason)?;
            state.emit(&out, Some(&|c| view::cancel_line(c, &out)))
        }
        Some("retry") => {
            let instructions = option_text(m, "--instructions");
            let context = human_context(None)?;
            let out = run_retry(
                state.app()?,
                &context,
                &mut LiveAgents,
                &attempt,
                instructions.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::retry_line(c, &out)))
        }
        Some("rerun") => {
            let context = human_context(None)?;
            let out = run_rerun(state.app()?, &context, &mut LiveAgents, &attempt)?;
            state.emit(&out, Some(&|c| view::rerun_line(c, &out)))
        }
        Some("schedule") => schedule(state, m),
        Some("decompose") => decompose(state, m),
        _ => Err(CliFailure::fail(
            wg_iface::cli::not_implemented_message(path),
            1,
        )),
    }
}

/// `run start`: queues the run, and with `--wait` follows it to its end.
fn start(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let task = option_text(m, "task").unwrap_or_default();
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let request = StartRequest {
        agent: option_text(m, "--agent"),
        model: option_text(m, "--model"),
        instructions: option_text(m, "--instructions"),
        ..StartRequest::default()
    };
    let context = human_context(None)?;
    let mut attempt = run_start(
        state.app()?,
        &context,
        &mut LiveAgents,
        workspace.as_deref(),
        &task,
        &request,
    )?;
    if option_flag(m, "--wait") {
        let id = get(&attempt, "id").map(python).unwrap_or_default();
        wait(state, &id)?;
        let run = runs::get(state.app()?, &id, 1)?;
        if let Some(found) = get(&run, "attempt") {
            attempt = found.clone();
        }
    }
    state.emit(&attempt, Some(&|c| view::start_line(c, &attempt)))
}

/// `run schedule`: one scheduler pass, the eligible tasks, or the reasons for one task.
fn schedule(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    if let Some(task) = option_text(m, "--explain") {
        let workspace = state.workspace_ref(None);
        let report = scheduler::explain(state.app()?, workspace.as_deref(), &task)?;
        let out = json_value(&report);
        return state.emit(&out, Some(&|c| view::explain_view(c, &out)));
    }
    if option_flag(m, "--dry-run") {
        let rows = scheduler::eligible_tasks(state.app()?, None, 50)?;
        let out = Value::List(rows.iter().map(|row| json_value(&row.to_json())).collect());
        return state.emit(&out, Some(&|c| view::eligible_view(c, &out)));
    }
    let report = scheduler::tick(state.app()?, &context, &mut LiveAgents)?;
    let out = json_value(&report);
    state.emit(&out, Some(&|c| view::tick_line(c, &out)))
}

/// `run decompose`: the model's child tasks for a task, created with `--create`.
fn decompose(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let task = option_text(m, "task").unwrap_or_default();
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let context = human_context(None)?;
    let id = scheduler::task_id_of(state.app()?, workspace.as_deref(), &task)?;
    let report = scheduler::decompose(
        state.app()?,
        &context,
        id,
        option_flag(m, "--create"),
        option_text(m, "--provider").as_deref(),
        option_text(m, "--model").as_deref(),
    )?;
    let out = json_value(&report);
    state.emit(&out, Some(&|c| view::decompose_view(c, &out)))
}

/// `run get`: the attempt, its job and its recent events.
fn show(state: &mut CliState, attempt: &str) -> Result<(), CliFailure> {
    let out = runs::get(state.app()?, attempt, RECENT_EVENTS)?;
    state.emit(&out, Some(&|c| view::get_view(c, &out)))
}

/// `_wait`: polls the attempt until it finishes, printing each new progress event as it arrives
/// (human output only). Fails with exit code 3 when the attempt does not finish in time.
fn wait(state: &mut CliState, attempt: &str) -> Result<(), CliFailure> {
    let deadline = Instant::now() + WATCH_TIMEOUT;
    let mut last_seq = 0;
    while Instant::now() < deadline {
        let run = runs::get(state.app()?, attempt, RECENT_EVENTS)?;
        let mut progress = String::new();
        for event in wg_iface::cli::args::items(get(&run, "recent_events")) {
            let seq = view::seq_of(event);
            if seq > last_seq {
                last_seq = seq;
                if !state.json {
                    progress.push_str(&state.console.print(&view::progress_markup(event)).text);
                }
            }
        }
        state.print(&progress);
        if view::finished(&run) {
            return Ok(());
        }
        std::thread::sleep(WATCH_INTERVAL);
    }
    Err(CliFailure::fail("timed out waiting for the run", 3))
}
