//! Bounded waits. Every wait the runner makes on an implementation (a CLI process, an MCP reply,
//! an HTTP response) gives up after a no-progress window: the process has produced no output, or
//! the socket has delivered no byte, for [`stall_timeout`]. A stalled wait becomes a step problem
//! (`stalled: ...`) instead of holding the whole run for a long timeout.
//!
//! The window is one process-wide setting, set once from `--stall-timeout` before any run starts,
//! so the many call sites that wait do not each carry it.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// The default no-progress window: long enough for a cold `uv run` to print, short enough that a
/// hung command costs seconds, not minutes.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a `serve` process may take to answer its health check before the run gives up. The
/// readiness loop also stops at once when the process exits.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

/// How often a waiting loop looks at the process and the activity channel.
pub(crate) const POLL: Duration = Duration::from_millis(50);

static STALL_MILLIS: AtomicU64 = AtomicU64::new(10_000);

/// The current no-progress window.
pub fn stall_timeout() -> Duration {
    Duration::from_millis(STALL_MILLIS.load(Ordering::Relaxed))
}

/// Set the no-progress window (`--stall-timeout`).
pub fn set_stall_timeout(window: Duration) {
    let millis = u64::try_from(window.as_millis()).unwrap_or(u64::MAX).max(1);
    STALL_MILLIS.store(millis, Ordering::Relaxed);
}

/// Parse a `--stall-timeout` value in seconds (fractions allowed), which must be positive.
pub fn parse_stall_timeout(text: &str) -> Result<Duration, String> {
    match text.parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs > 0.0 => {
            Duration::try_from_secs_f64(secs).map_err(|e| format!("--stall-timeout `{text}`: {e}"))
        }
        _ => Err(format!(
            "--stall-timeout needs a positive number of seconds, not `{text}`"
        )),
    }
}

/// What the reader threads of a child's pipes report.
enum Event {
    /// Bytes arrived on one of the pipes.
    Activity,
    /// One of the pipes reached end of file.
    Closed,
}

/// Run a command to completion and collect its output, like `Command::output` with optional stdin,
/// but give up when the process prints nothing for [`stall_timeout`]. A stalled process is killed
/// (with its tree on Windows) and the error says so. A process that exits while a grandchild still
/// holds its pipes is not waited for beyond that same window.
pub fn run_bounded(
    cmd: &mut Command,
    stdin: Option<&str>,
) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    run_bounded_with(cmd, stdin, stall_timeout())
}

fn run_bounded_with(
    cmd: &mut Command,
    stdin: Option<&str>,
    window: Duration,
) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let text = text.to_owned();
        // Written from a thread: a child that never reads its stdin must not hold the wait.
        thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let (tx, rx) = mpsc::channel::<Event>();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = spawn_reader(stdout, tx.clone());
    let err_reader = spawn_reader(stderr, tx.clone());
    drop(tx);

    let mut last_activity = Instant::now();
    let mut closed = 0;
    let mut status = None;
    loop {
        if crate::budget::cut() {
            kill_tree(&mut child);
            return Err(crate::budget::cut_error());
        }
        if status.is_none() {
            status = child.try_wait().map_err(|e| format!("wait failed: {e}"))?;
        }
        if status.is_some() && closed == 2 {
            break;
        }
        if last_activity.elapsed() >= window {
            kill_tree(&mut child);
            return Err(format!(
                "stalled: no output for {:.1} s; the process was killed",
                window.as_secs_f64()
            ));
        }
        match rx.recv_timeout(POLL) {
            Ok(Event::Activity) => last_activity = Instant::now(),
            Ok(Event::Closed) => closed += 1,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(POLL),
        }
    }
    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    let out = join_reader(out_reader);
    let err = join_reader(err_reader);
    Ok((code, out, err))
}

/// A thread that reads one pipe to its end, reporting each chunk as activity.
fn spawn_reader(
    pipe: Option<impl Read + Send + 'static>,
    tx: mpsc::Sender<Event>,
) -> Option<thread::JoinHandle<Vec<u8>>> {
    let mut pipe = pipe?;
    Some(thread::spawn(move || {
        let mut all = Vec::new();
        let mut buf = [0_u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    let _ = tx.send(Event::Activity);
                }
            }
        }
        let _ = tx.send(Event::Closed);
        all
    }))
}

fn join_reader(reader: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// Kill a stalled child. On Windows the whole tree goes, since a `uv run` leaves its Python child
/// behind on a plain kill.
fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_values_must_be_positive_seconds() {
        assert_eq!(
            parse_stall_timeout("2.5").unwrap(),
            Duration::from_millis(2500)
        );
        assert!(parse_stall_timeout("0").is_err());
        assert!(parse_stall_timeout("-1").is_err());
        assert!(parse_stall_timeout("soon").is_err());
    }

    #[test]
    fn a_quiet_process_is_killed_after_the_window() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > nul"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        let started = Instant::now();
        let err = run_bounded_with(&mut cmd, None, Duration::from_millis(300)).unwrap_err();
        assert!(err.starts_with("stalled:"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn output_keeps_the_wait_alive_and_is_collected() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo hello"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "echo hello; echo oops 1>&2; exit 3"]);
            c
        };
        let (code, out, err) = run_bounded_with(&mut cmd, None, Duration::from_secs(5)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hello");
        if cfg!(windows) {
            assert_eq!(code, 0);
        } else {
            assert_eq!(code, 3);
            assert_eq!(String::from_utf8_lossy(&err).trim(), "oops");
        }
    }
}
