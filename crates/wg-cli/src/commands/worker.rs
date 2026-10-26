//! `workgraph worker run, list, recover` (IF-11): the background worker and its registry.
//!
//! `worker run` installs the interrupt handlers before it claims anything: Ctrl+C (and SIGTERM on
//! Unix) sets the worker's stop flag, so the worker stops claiming, gives its running jobs their
//! grace period and records its own stop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::ArgMatches;
use wg_clispec::job as view;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{
    HandlerRegistry, RunOptions, Worker, WorkerOptions, human_context, worker_summary_out,
};

use super::Group;
use crate::CliState;

/// The `worker` group.
pub const GROUP: Group = Group {
    name: "worker",
    command: view::worker_command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("run") => run_worker(state, m),
        Some("list") => {
            let out = state.app()?.worker_list()?;
            state.emit(&out, Some(&|c| view::worker_list_view(c, &out)))
        }
        Some("recover") => {
            let context = human_context(None)?;
            let out = state.app()?.worker_recover(&context)?;
            state.emit(&out, Some(&|c| view::recovery_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `worker run`: runs until the queue is drained (`--once`), idle for `--max-idle` seconds, or
/// interrupted.
fn run_worker(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    state.configure_logging()?;
    let options = WorkerOptions {
        home: state.home.clone(),
        concurrency: view::concurrency(m)?,
        kinds: view::worker_kinds(m),
        ..WorkerOptions::default()
    };
    let mut worker = Worker::open(options, HandlerRegistry::standard())?;
    install_interrupt_handlers(worker.stop_flag());
    let started = view::started_line(&state.console, &worker.worker_id, worker.concurrency);
    state.print(&started);
    let run = RunOptions {
        once: view::once(m),
        max_idle: view::max_idle(m)?.map(Duration::from_secs_f64),
        grace: None,
    };
    let summary = worker.run(run)?;
    let out = worker_summary_out(&worker.worker_id, summary.completed, summary.failed);
    state.emit(&out, Some(&|c| view::stopped_line(c, &out)))
}

/// Sets `stop` when the process is interrupted. The waiting thread is left to the process: it
/// ends with the command.
fn install_interrupt_handlers(stop: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new()
        .name("wg-interrupt".to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                #[cfg(unix)]
                if let Ok(mut terminate) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = terminate.recv() => {}
                    }
                    stop.store(true, Ordering::SeqCst);
                    return;
                }
                let _ = tokio::signal::ctrl_c().await;
                stop.store(true, Ordering::SeqCst);
            });
        });
    // A thread that cannot start leaves the worker to run until it drains or is killed.
    drop(spawned);
}
