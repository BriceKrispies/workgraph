//! Real-process tests for wg-proc.
//!
//! This target has `harness = false`: when the environment variable `WG_PROC_HELPER` is set the
//! binary acts as the child process (modes below), otherwise it runs the tests, spawning itself
//! as the child. That keeps the tests fast and independent of any external tool.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wg_proc::{
    CaptureLimits, LineOptions, ProcError, ProcessSpec, base_environment_from, run_capture,
    run_process,
};

const HELPER: &str = "WG_PROC_HELPER";
const ARG: &str = "WG_PROC_ARG";

// ---------------------------------------------------------------------------------------------
// The child side.
// ---------------------------------------------------------------------------------------------

fn helper(mode: &str) -> i32 {
    let arg = std::env::var(ARG).unwrap_or_default();
    match mode {
        "lines" => {
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            let _ = writeln!(out, "first");
            let _ = out.flush();
            let _ = writeln!(err, "second");
            let _ = err.flush();
            let _ = write!(out, "third\r\npartial");
            let _ = out.flush();
            0
        }
        "exit" => arg.parse().unwrap_or(2),
        "stdin" => {
            let mut data = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut data);
            println!(
                "got {} bytes: {}",
                data.len(),
                String::from_utf8_lossy(&data)
            );
            0
        }
        "env" => {
            for name in ["SECRET_TOKEN", "LANG", "KEPT_BY_EXTRA"] {
                println!(
                    "{name}={}",
                    std::env::var(name).unwrap_or_else(|_| "<unset>".into())
                );
            }
            0
        }
        "pwd" => {
            println!(
                "{}",
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
            0
        }
        "sleep" => {
            std::thread::sleep(Duration::from_secs(120));
            0
        }
        "tree" => {
            let child = Command::new(std::env::current_exe().unwrap_or_default())
                .env(HELPER, "sleep")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn();
            if let Ok(child) = child {
                println!("grandchild {}", child.id());
                let _ = std::io::stdout().flush();
                std::thread::sleep(Duration::from_secs(120));
            }
            0
        }
        "split" => {
            // stdout then stderr, with a known amount of each.
            let n: usize = arg.parse().unwrap_or(0);
            let _ = std::io::stdout().write_all(&vec![b'o'; n]);
            let _ = std::io::stderr().write_all(&vec![b'e'; n]);
            0
        }
        "flood" => {
            let n: usize = arg.parse().unwrap_or(0);
            let mut out = std::io::BufWriter::new(std::io::stdout());
            for i in 0..n {
                let _ = writeln!(out, "line {i}");
            }
            let _ = out.flush();
            0
        }
        "longline" => {
            let n: usize = arg.parse().unwrap_or(0);
            let _ = std::io::stdout().write_all(&vec![b'x'; n]);
            let _ = std::io::stdout().write_all(b"\nshort\n");
            0
        }
        _ => 99,
    }
}

// ---------------------------------------------------------------------------------------------
// The parent side.
// ---------------------------------------------------------------------------------------------

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// A spec that runs this binary as the helper `mode` with a child environment holding only the
/// helper variables.
fn spec(mode: &str, arg: &str) -> ProcessSpec {
    let mut spec = ProcessSpec::new([exe()]);
    let mut env: BTreeMap<OsString, OsString> = BTreeMap::new();
    env.insert(HELPER.into(), mode.into());
    env.insert(ARG.into(), arg.into());
    spec.env = Some(env);
    spec.poll_interval_ms = 10;
    spec.drain_timeout_ms = 10_000;
    spec
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-proc-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lines_merge_both_streams_and_stream_to_callback_and_log() {
    let dir = scratch("lines");
    let log = dir.join("nested").join("run.log");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_in = Arc::clone(&seen);
    let options = LineOptions {
        on_line: Some(Box::new(move |line| {
            seen_in.lock().unwrap().push(line.to_owned())
        })),
        log_file: Some(log.clone()),
        ..LineOptions::default()
    };
    let result = run_process(&spec("lines", ""), options, None).unwrap();
    assert!(result.ok(), "{result:?}");
    assert_eq!(result.tail, ["first", "second", "third", "partial"]);
    assert_eq!(result.lines, 4);
    assert_eq!(*seen.lock().unwrap(), result.tail);
    assert!(!result.output_incomplete);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "first\nsecond\nthird\npartial\n"
    );

    // The log is appended to, not replaced.
    let options = LineOptions {
        log_file: Some(log.clone()),
        ..LineOptions::default()
    };
    run_process(&spec("lines", ""), options, None).unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 8);
}

fn exit_codes_are_reported() {
    let result = run_process(&spec("exit", "7"), LineOptions::default(), None).unwrap();
    assert_eq!(result.exit_code, Some(7));
    assert!(!result.ok());
    assert!(!result.timed_out && !result.cancelled);
}

fn stdin_is_fed_and_closed() {
    let mut s = spec("stdin", "");
    s.stdin_data = Some(b"hello stdin".to_vec());
    let result = run_process(&s, LineOptions::default(), None).unwrap();
    assert_eq!(result.tail, ["got 11 bytes: hello stdin"]);

    // A large input is written without deadlocking against the output pipe.
    let mut s = spec("stdin", "");
    s.stdin_data = Some(vec![b'z'; 1 << 20]);
    let result = run_capture(&s, CaptureLimits::default(), None).unwrap();
    assert!(String::from_utf8_lossy(&result.stdout).starts_with("got 1048576 bytes"));

    // Without stdin data the child sees an empty, closed stdin.
    let result = run_process(&spec("stdin", ""), LineOptions::default(), None).unwrap();
    assert_eq!(result.tail, ["got 0 bytes: "]);
}

fn minimal_environment_hides_secrets() {
    let parent = vec![
        (OsString::from("SECRET_TOKEN"), OsString::from("hunter2")),
        (OsString::from("LANG"), OsString::from("C")),
    ];
    let mut env = base_environment_from(parent, [("KEPT_BY_EXTRA", "yes")], false);
    env.insert(HELPER.into(), "env".into());
    let mut s = ProcessSpec::new([exe()]);
    s.env = Some(env);
    let result = run_process(&s, LineOptions::default(), None).unwrap();
    assert_eq!(
        result.tail,
        ["SECRET_TOKEN=<unset>", "LANG=C", "KEPT_BY_EXTRA=yes"]
    );
}

fn cwd_is_applied() {
    let dir = scratch("cwd");
    let mut s = spec("pwd", "");
    s.cwd = Some(dir.clone());
    let result = run_process(&s, LineOptions::default(), None).unwrap();
    let reported = PathBuf::from(&result.tail[0]);
    assert_eq!(
        reported.canonicalize().unwrap(),
        dir.canonicalize().unwrap()
    );
}

fn timeout_terminates_the_child() {
    let mut s = spec("sleep", "");
    s.timeout_ms = Some(300);
    let started = Instant::now();
    let result = run_process(&s, LineOptions::default(), None).unwrap();
    assert!(result.timed_out && !result.ok(), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!result.output_incomplete);
}

fn cancellation_terminates_the_child() {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&flag);
    let timer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        setter.store(true, Ordering::SeqCst);
    });
    let probe = || flag.load(Ordering::SeqCst);
    let started = Instant::now();
    let result = run_process(&spec("sleep", ""), LineOptions::default(), Some(&probe)).unwrap();
    timer.join().unwrap();
    assert!(
        result.cancelled && !result.timed_out && !result.ok(),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}

fn timeout_kills_grandchildren_too() {
    let mut s = spec("tree", "");
    s.timeout_ms = Some(1500);
    s.drain_timeout_ms = 20_000;
    let started = Instant::now();
    let result = run_process(&s, LineOptions::default(), None).unwrap();
    assert!(result.timed_out, "{result:?}");
    // The grandchild holds the output pipe: had it survived, draining would have waited for it.
    assert!(!result.output_incomplete, "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    let pid: u32 = result
        .tail
        .iter()
        .find_map(|l| l.strip_prefix("grandchild "))
        .and_then(|p| p.trim().parse().ok())
        .expect("the child reports its grandchild's pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "grandchild {pid} is still running"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain FFI calls; the handle is checked for null, used only for the query and closed
    // exactly once; `code` is a valid out pointer for the duration of the call.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0_u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 only checks that the process exists.
    i32::try_from(pid).is_ok_and(|p| unsafe { libc::kill(p, 0) } == 0)
}

fn capture_separates_and_bounds_streams() {
    let result = run_capture(&spec("split", "100"), CaptureLimits::default(), None).unwrap();
    assert_eq!(result.stdout, vec![b'o'; 100]);
    assert_eq!(result.stderr, vec![b'e'; 100]);
    assert!(!result.stdout_truncated && !result.stderr_truncated);

    let limits = CaptureLimits {
        max_stdout_bytes: 10,
        max_stderr_bytes: 1000,
    };
    let result = run_capture(&spec("split", "100"), limits, None).unwrap();
    assert_eq!(result.stdout, vec![b'o'; 10]);
    assert!(result.stdout_truncated && !result.stderr_truncated);
    assert!(result.ok());
}

fn tail_and_line_length_are_bounded() {
    let options = LineOptions {
        tail_lines: 5,
        ..LineOptions::default()
    };
    let result = run_process(&spec("flood", "20000"), options, None).unwrap();
    assert_eq!(result.lines, 20000);
    assert_eq!(result.tail.len(), 5);
    assert_eq!(result.tail[4], "line 19999");

    let options = LineOptions {
        max_line_bytes: 100,
        ..LineOptions::default()
    };
    let result = run_process(&spec("longline", "100000"), options, None).unwrap();
    assert_eq!(result.tail, ["x".repeat(100), "short".to_owned()]);
    assert_eq!(result.truncated_lines, 1);
}

fn errors_are_reported_not_panicked() {
    let empty = ProcessSpec::new(Vec::<String>::new());
    assert!(matches!(
        run_process(&empty, LineOptions::default(), None),
        Err(ProcError::EmptyArgv)
    ));
    let missing = ProcessSpec::new(["wg-proc-definitely-not-a-program"]);
    assert!(matches!(
        run_process(&missing, LineOptions::default(), None),
        Err(ProcError::NotFound(_))
    ));
    assert!(matches!(
        run_capture(&missing, CaptureLimits::default(), None),
        Err(ProcError::NotFound(_))
    ));
    let bad_name = ProcessSpec::new(["a\0b"]);
    assert!(matches!(
        run_process(&bad_name, LineOptions::default(), None),
        Err(ProcError::InvalidExecutable)
    ));
    let mut bad_cwd = spec("exit", "0");
    bad_cwd.cwd = Some(PathBuf::from("/wg-proc/no/such/dir"));
    assert!(matches!(
        run_process(&bad_cwd, LineOptions::default(), None),
        Err(ProcError::Spawn { .. })
    ));
}

fn main() {
    if let Ok(mode) = std::env::var(HELPER) {
        std::process::exit(helper(&mode));
    }
    let tests: &[(&str, fn())] = &[
        (
            "lines_merge_both_streams_and_stream_to_callback_and_log",
            lines_merge_both_streams_and_stream_to_callback_and_log,
        ),
        ("exit_codes_are_reported", exit_codes_are_reported),
        ("stdin_is_fed_and_closed", stdin_is_fed_and_closed),
        (
            "minimal_environment_hides_secrets",
            minimal_environment_hides_secrets,
        ),
        ("cwd_is_applied", cwd_is_applied),
        ("timeout_terminates_the_child", timeout_terminates_the_child),
        (
            "cancellation_terminates_the_child",
            cancellation_terminates_the_child,
        ),
        (
            "timeout_kills_grandchildren_too",
            timeout_kills_grandchildren_too,
        ),
        (
            "capture_separates_and_bounds_streams",
            capture_separates_and_bounds_streams,
        ),
        (
            "tail_and_line_length_are_bounded",
            tail_and_line_length_are_bounded,
        ),
        (
            "errors_are_reported_not_panicked",
            errors_are_reported_not_panicked,
        ),
    ];
    let mut failed = 0;
    for (name, test) in tests {
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(test));
        let status = if outcome.is_ok() { "ok" } else { "FAILED" };
        println!(
            "test {name} ... {status} ({} ms)",
            started.elapsed().as_millis()
        );
        failed += usize::from(outcome.is_err());
    }
    println!("\n{} passed; {failed} failed", tests.len() - failed);
    if failed > 0 {
        std::process::exit(1);
    }
}
