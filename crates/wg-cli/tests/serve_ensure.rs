//! WG-5702: `serve --ensure` leaves exactly one daemon when two invocations race, and a third
//! invocation finds it running and starts nothing.
//!
//! The invocations write their output to files, not pipes. A daemon inherits the inheritable
//! handles of the process that starts it, so a pipe would stay open for as long as the daemon
//! runs, and reading the output to its end would wait for the daemon too. The test process also
//! stops passing its own standard handles to its children, because under `cargo test` they are
//! the pipe `cargo` reads.
//!
//! Nothing panics before the daemons are stopped: every invocation returns its result, and the
//! assertions run last.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use wg_proc::Spawn;

/// Stops this process's standard handles from being inherited by the processes it starts.
#[cfg(windows)]
fn stop_inheriting_std_handles() {
    use std::ffi::c_void;
    const STD_HANDLES: [u32; 3] = [u32::MAX - 9, u32::MAX - 10, u32::MAX - 11];
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn SetHandleInformation(object: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    for id in STD_HANDLES {
        // SAFETY: `GetStdHandle` only reads its argument; `SetHandleInformation` changes the
        // inherit flag of the handle given and fails harmlessly for a missing one.
        unsafe {
            SetHandleInformation(GetStdHandle(id), HANDLE_FLAG_INHERIT, 0);
        }
    }
}

#[cfg(not(windows))]
fn stop_inheriting_std_handles() {}

/// A loopback port nothing listens on right now.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    port
}

/// One invocation's result: its exit success, its answer text and, when it parses, its status and
/// process id.
struct Run {
    success: bool,
    answer: String,
    status: Option<String>,
    pid: Option<u64>,
    version: Option<String>,
}

/// Runs `workgraph --home HOME --json serve --ensure --port PORT`, with its output in `out`.
fn ensure(exe: &Path, home: &Path, port: u16, out: &Path) -> Run {
    let stdout = File::create(out).expect("an output file");
    let success = Spawn::new(exe)
        .arg("--home")
        .arg(home)
        .args(["--json", "serve", "--ensure", "--port"])
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    let answer = std::fs::read_to_string(out).unwrap_or_default();
    let value: Option<serde_json::Value> = serde_json::from_str(&answer).ok();
    Run {
        success,
        answer,
        status: value
            .as_ref()
            .and_then(|v| v["status"].as_str().map(str::to_owned)),
        pid: value.as_ref().and_then(|v| v["pid"].as_u64()),
        version: value
            .as_ref()
            .and_then(|v| v["version"].as_str().map(str::to_owned)),
    }
}

/// True when `GET /api/v1/health` on the loopback port answers 200.
fn answers(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    if stream
        .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok();
    reply.starts_with("HTTP/1.1 200")
}

/// Ends a daemon this test started, by process id.
fn kill(pid: u64) {
    let pid = pid.to_string();
    if cfg!(windows) {
        Spawn::new("taskkill")
            .args(["/PID", &pid, "/F"])
            .output()
            .ok();
    } else {
        Spawn::new("kill").args(["-9", &pid]).output().ok();
    }
}

#[test]
fn racing_ensures_leave_one_daemon_and_a_later_one_starts_nothing() {
    stop_inheriting_std_handles();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_workgraph"));
    let scratch = std::env::temp_dir().join(format!("wg-ensure-{}", std::process::id()));
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).expect("a scratch home");
    let port = free_port();

    let racers: Vec<_> = (0..2)
        .map(|index| {
            let exe = exe.clone();
            let home = home.clone();
            let out = scratch.join(format!("racer-{index}.json"));
            std::thread::spawn(move || ensure(&exe, &home, port, &out))
        })
        .collect();
    let runs: Vec<Run> = racers
        .into_iter()
        .map(|racer| racer.join().expect("the racer thread finishes"))
        .collect();
    let later = ensure(&exe, &home, port, &scratch.join("later.json"));
    let answered = answers(port);

    // Stop what the test started before anything can fail.
    for run in runs.iter().chain(std::iter::once(&later)) {
        if let (Some("started"), Some(pid)) = (run.status.as_deref(), run.pid) {
            kill(pid);
        }
    }
    std::fs::remove_dir_all(&scratch).ok();

    let describe: Vec<&str> = runs.iter().map(|run| run.answer.trim()).collect();
    assert!(
        runs.iter().all(|run| run.success),
        "both racers exit 0: {describe:?}"
    );
    let started = runs
        .iter()
        .filter(|run| run.status.as_deref() == Some("started"))
        .count();
    let running = runs
        .iter()
        .filter(|run| run.status.as_deref() == Some("running"))
        .count();
    assert_eq!(
        started, 1,
        "exactly one racer started the server: {describe:?}"
    );
    assert_eq!(running, 1, "the other racer found it running: {describe:?}");
    assert!(answered, "the started server answers its health probe");
    let started_version = runs
        .iter()
        .find(|run| run.status.as_deref() == Some("started"))
        .and_then(|run| run.version.clone());
    assert!(
        started_version
            .as_deref()
            .is_some_and(|v| !v.is_empty() && v != "unknown"),
        "the started daemon reports its real version: {describe:?}"
    );
    assert!(later.success, "a later invocation exits 0");
    assert_eq!(
        later.status.as_deref(),
        Some("running"),
        "a later invocation starts nothing"
    );
    assert_eq!(later.pid, None, "a later invocation reports no process id");
}
