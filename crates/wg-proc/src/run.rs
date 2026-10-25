//! The runner: spawn, feed stdin, read output, enforce timeout and cancellation.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use wg_procspec::{
    CaptureLimits, CaptureResult, LineOptions, ProcError, ProcessResult, ProcessSpec,
    exit_code_from_parts,
};

use crate::resolve::resolve_executable;
use crate::sink::{ByteBuffer, LineSink, Sink};
use crate::tree::{self, TreeGuard};

/// A cancellation probe, polled every [`ProcessSpec::poll_interval_ms`]. Returning `true` terminates
/// the child's tree.
pub type Cancel<'a> = &'a (dyn Fn() -> bool + 'a);

/// Run a child with stdout and stderr merged into one stream consumed line by line.
///
/// # Errors
/// [`ProcError`] when the program cannot be resolved or started, the log file cannot be opened or
/// waiting on the child fails. A child that exits non-zero, times out or is cancelled is a normal
/// [`ProcessResult`].
pub fn run_process(
    spec: &ProcessSpec,
    options: LineOptions,
    cancel: Option<Cancel<'_>>,
) -> Result<ProcessResult, ProcError> {
    let LineOptions {
        on_line,
        log_file,
        tail_lines,
        max_line_bytes,
    } = options;
    let log = match log_file {
        Some(path) => Some(open_log(&path)?),
        None => None,
    };
    let sink = Arc::new(Mutex::new(LineSink::new(
        max_line_bytes,
        tail_lines,
        log,
        on_line,
    )));
    let as_sink: Arc<Mutex<dyn Sink>> = sink.clone();
    let outcome = execute(spec, &as_sink, None, cancel)?;
    let sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(ProcessResult {
        argv: outcome.argv,
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
        timed_out: outcome.timed_out,
        cancelled: outcome.cancelled,
        tail: sink.tail.iter().cloned().collect(),
        lines: sink.lines,
        truncated_lines: sink.truncated_lines,
        output_incomplete: outcome.output_incomplete,
    })
}

/// Run a child and capture stdout and stderr separately, each bounded in memory.
///
/// # Errors
/// As [`run_process`], except there is no log file.
pub fn run_capture(
    spec: &ProcessSpec,
    limits: CaptureLimits,
    cancel: Option<Cancel<'_>>,
) -> Result<CaptureResult, ProcError> {
    let out = Arc::new(Mutex::new(ByteBuffer {
        limit: limits.max_stdout_bytes,
        data: Vec::new(),
        truncated: false,
    }));
    let err = Arc::new(Mutex::new(ByteBuffer {
        limit: limits.max_stderr_bytes,
        data: Vec::new(),
        truncated: false,
    }));
    let out_sink: Arc<Mutex<dyn Sink>> = out.clone();
    let err_sink: Arc<Mutex<dyn Sink>> = err.clone();
    let outcome = execute(spec, &out_sink, Some(&err_sink), cancel)?;
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    let mut err = err.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(CaptureResult {
        argv: outcome.argv,
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
        timed_out: outcome.timed_out,
        cancelled: outcome.cancelled,
        stdout: std::mem::take(&mut out.data),
        stderr: std::mem::take(&mut err.data),
        stdout_truncated: out.truncated,
        stderr_truncated: err.truncated,
        output_incomplete: outcome.output_incomplete,
    })
}

fn open_log(path: &std::path::Path) -> Result<std::fs::File, ProcError> {
    let fail = |source| ProcError::LogFile {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(fail)
}

struct Outcome {
    argv: Vec<String>,
    exit_code: Option<i32>,
    duration_ms: u64,
    timed_out: bool,
    cancelled: bool,
    output_incomplete: bool,
}

/// The shared engine. With `err_sink` `None`, stderr is merged into stdout.
fn execute(
    spec: &ProcessSpec,
    out_sink: &Arc<Mutex<dyn Sink>>,
    err_sink: Option<&Arc<Mutex<dyn Sink>>>,
    cancel: Option<Cancel<'_>>,
) -> Result<Outcome, ProcError> {
    let Some(first) = spec.argv.first() else {
        return Err(ProcError::EmptyArgv);
    };
    let program = resolve_executable(first)?;
    let mut argv = vec![program.display().to_string()];
    argv.extend(spec.argv[1..].iter().cloned());

    let mut command = std::process::Command::new(&program);
    command.args(&spec.argv[1..]);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = &spec.env {
        command.env_clear().envs(env);
    }
    command.stdin(if spec.stdin_data.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let spawn_error = |source| ProcError::Spawn {
        program: first.clone(),
        source,
    };
    let merged_reader = if err_sink.is_some() {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        None
    } else {
        let (reader, writer) = io::pipe().map_err(spawn_error)?;
        command
            .stdout(writer.try_clone().map_err(spawn_error)?)
            .stderr(writer);
        Some(reader)
    };
    tree::configure(&mut command);

    let started = Instant::now();
    let mut child = command.spawn().map_err(spawn_error)?;
    // The command holds the pipe write ends; dropping it lets the readers see end of file.
    drop(command);
    let guard = TreeGuard::attach(&child);

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let mut readers = 0_usize;
    let mut start_reader = |pipe: Box<dyn Read + Send>, sink: &Arc<Mutex<dyn Sink>>| {
        readers += 1;
        let sink = Arc::clone(sink);
        let done = done_tx.clone();
        thread::Builder::new()
            .name("wg-proc-reader".into())
            .spawn(move || {
                pump(pipe, &sink);
                let _ = done.send(());
            })
    };
    let reader_result = match (merged_reader, err_sink) {
        (Some(merged), _) => start_reader(Box::new(merged), out_sink).map(|_| ()),
        (None, Some(err_sink)) => match (child.stdout.take(), child.stderr.take()) {
            (Some(out), Some(err)) => start_reader(Box::new(out), out_sink)
                .and_then(|_| start_reader(Box::new(err), err_sink))
                .map(|_| ()),
            _ => Ok(()),
        },
        (None, None) => Ok(()),
    };
    drop(done_tx);
    if let Err(source) = reader_result {
        let _ = guard.kill_tree(&mut child);
        return Err(ProcError::Spawn {
            program: first.clone(),
            source,
        });
    }

    if let (Some(data), Some(mut stdin)) = (spec.stdin_data.clone(), child.stdin.take()) {
        // Written on its own thread so a child that does not read cannot block us. The thread ends
        // when the data is written or the pipe breaks (the child exited or was killed).
        let _ = thread::Builder::new()
            .name("wg-proc-stdin".into())
            .spawn(move || {
                let _ = stdin.write_all(&data);
                let _ = stdin.flush();
            });
    }

    let timeout = spec.timeout_ms.map(Duration::from_millis);
    let poll_interval = Duration::from_millis(spec.poll_interval_ms);
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = guard.kill_tree(&mut child);
                return Err(ProcError::Wait(error));
            }
        }
        let elapsed = started.elapsed();
        let remaining = timeout.map(|limit| limit.saturating_sub(elapsed));
        if remaining.is_some_and(|r| r.is_zero()) {
            timed_out = true;
        } else if cancel.is_some_and(|probe| probe()) {
            cancelled = true;
        }
        if timed_out || cancelled {
            guard.kill_tree(&mut child).map_err(ProcError::Wait)?;
            break child.wait().map_err(ProcError::Wait)?;
        }
        let nap = remaining.map_or(poll_interval, |r| r.min(poll_interval));
        thread::sleep(nap.max(Duration::from_millis(1)));
    };

    let mut waiting = readers;
    let output_incomplete = !wait_for_readers(
        &done_rx,
        &mut waiting,
        Duration::from_millis(spec.drain_timeout_ms),
    );
    if output_incomplete {
        // A descendant outside the tree, or one that survived, still holds the pipes.
        let _ = guard.kill_tree(&mut child);
        wait_for_readers(&done_rx, &mut waiting, Duration::from_secs(2));
    }

    Ok(Outcome {
        argv,
        exit_code: exit_code_from_parts(status.code(), signal_of(&status)),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        timed_out,
        cancelled,
        output_incomplete,
    })
}

/// Read a pipe until end of file, feeding the sink.
fn pump(mut pipe: Box<dyn Read + Send>, sink: &Arc<Mutex<dyn Sink>>) {
    let mut buffer = [0_u8; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(&buffer[..n]),
        }
    }
    sink.lock().unwrap_or_else(PoisonError::into_inner).finish();
}

/// Wait until `remaining` readers have reported, or `limit` has passed. `true` if all reported.
fn wait_for_readers(done: &mpsc::Receiver<()>, remaining: &mut usize, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while *remaining > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        match done.recv_timeout(left) {
            Ok(()) => *remaining -= 1,
            // Disconnected means every reader thread has ended (a panicked one included).
            Err(RecvTimeoutError::Disconnected) => {
                *remaining = 0;
            }
            Err(RecvTimeoutError::Timeout) => return false,
        }
    }
    true
}

fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}
