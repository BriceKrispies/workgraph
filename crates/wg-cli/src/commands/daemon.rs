//! The single long-running `workgraph serve` daemon (WG-5702).
//!
//! `serve --ensure` starts `workgraph serve` detached only when nothing answers the health probe
//! on the configured address, then returns; two invocations that race leave one server, because
//! the second server cannot bind the port and exits. `serve --restart` asks the running daemon to
//! stop through a stop-request file it polls, waits until it stops answering, and starts one from
//! this binary. `ui open` starts its server with the same detached start, so the code lives here.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use clap::ArgMatches;
use tokio_util::sync::CancellationToken;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{option_flag, option_text};
use wg_iface::http::serve::Settings;
use wg_json::{PyStr, Value, dict};
use wg_runtime::{config_at, resolve_paths};
use wg_uispec::Wait;

use crate::CliState;
use crate::spec::servers::port_option;
use crate::spec::ui::base_url;
use wg_proc::Spawn;

/// The file a restart writes into the data directory. The running daemon removes it and stops.
pub const STOP_FILE: &str = "serve.stop";

/// The lock file `serve --ensure` holds while it probes and starts the daemon (WG-5714).
const ENSURE_LOCK_FILE: &str = "serve.ensure.lock";

/// How often the stop file is looked for, and a started server is probed while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Seconds a restart waits for the running daemon to stop answering: the worker join of `serve` (40 seconds)
/// and a margin.
const STOP_TIMEOUT: f64 = 60.0;

/// Seconds a start waits for the server it launched to answer.
const START_TIMEOUT: f64 = 20.0;

/// One health probe: true when the server answers with a success status.
pub fn answering(probe: &wg_uispec::Probe) -> bool {
    wg_net::http_status_line(
        &probe.authority,
        &wg_uispec::request_text(probe),
        Duration::from_secs_f64(wg_uispec::PROBE_TIMEOUT),
    )
    .is_ok_and(|line| wg_uispec::answers_line(&line))
}

/// Starts `workgraph serve` detached and polls it until it answers, the process ends or the
/// timeout passes. Returns the started server's process id.
pub fn start_and_wait(
    home: Option<&Path>,
    listen: Option<(&str, u16)>,
    log: &Path,
    probe: &wg_uispec::Probe,
    base: &str,
    timeout: f64,
) -> Result<u32, CliFailure> {
    let log_text = log.display().to_string();
    let mut child = spawn_serve(home, listen, log)
        .map_err(|err| CliFailure::fail(format!("cannot start `workgraph serve`: {err}"), 1))?;
    let pid = child.id();
    let started_at = Instant::now();
    loop {
        let up = answering(probe);
        let exited = child
            .try_wait()
            .map_err(|err| CliFailure::fail(format!("cannot watch `workgraph serve`: {err}"), 1))?
            .map(|status| status.code().unwrap_or(1));
        let elapsed = started_at.elapsed().as_secs_f64();
        match wg_uispec::wait_step(up, exited, elapsed, timeout) {
            Wait::Ready => return Ok(pid),
            Wait::Keep => std::thread::sleep(POLL_INTERVAL),
            Wait::Exited(code) => {
                return Err(CliFailure::fail(
                    wg_uispec::exited_message(code, &log_text),
                    1,
                ));
            }
            Wait::TimedOut => {
                return Err(CliFailure::fail(
                    wg_uispec::timed_out_message(base, timeout, &log_text),
                    1,
                ));
            }
        }
    }
}

/// Launches this binary's `serve` with its output appended to `log`. The server gets its own
/// process group, and on Windows a hidden console (see `wg_proc::Spawn`), so the consoles of the
/// children it starts stay hidden too.
pub fn spawn_serve(
    home: Option<&Path>,
    listen: Option<(&str, u16)>,
    log: &Path,
) -> std::io::Result<Child> {
    let exe = std::env::current_exe()?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let errors = file.try_clone()?;
    let home_text = home.map(|path| path.to_string_lossy().into_owned());
    let mut args = wg_uispec::serve_args(home_text.as_deref());
    if let Some((host, port)) = listen {
        args.extend(["--host".to_owned(), host.to_owned()]);
        args.extend(["--port".to_owned(), port.to_string()]);
    }
    #[cfg(windows)]
    stop_std_handles_inheritance();
    let mut command = Spawn::new(exe);
    command.own_process_group().force_hidden_console();
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors));
    command.spawn()
}

/// Stops this process's standard handles from being inherited by children (Windows). The daemon
/// is started with its own stdio, but Windows also hands it every inheritable handle of this
/// process: a `serve --ensure` whose output is a pipe would leave the daemon holding that pipe, so
/// a caller waiting for the end of the output would wait for the daemon too (WG-5702).
#[cfg(windows)]
fn stop_std_handles_inheritance() {
    use std::ffi::c_void;
    /// `STD_INPUT_HANDLE`, `STD_OUTPUT_HANDLE` and `STD_ERROR_HANDLE` are `(DWORD)-10`, `-11`, `-12`.
    const STD_HANDLES: [u32; 3] = [u32::MAX - 9, u32::MAX - 10, u32::MAX - 11];
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn SetHandleInformation(object: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    for id in STD_HANDLES {
        // SAFETY: `GetStdHandle` only reads its argument. `SetHandleInformation` changes only the
        // inherit flag of the handle it is given; a missing or invalid handle makes it fail, which
        // is ignored.
        unsafe {
            SetHandleInformation(GetStdHandle(id), HANDLE_FLAG_INHERIT, 0);
        }
    }
}

/// The address the `serve` options name: `--host` and `--port`, else the configuration.
fn daemon_listen(state: &CliState, m: &ArgMatches) -> Result<(String, u16), CliFailure> {
    let settings = Settings::from_config(&config_at(&resolve_paths(state.home.as_deref())?)?);
    let host = option_text(m, "--host").unwrap_or_else(|| settings.host.clone());
    let port = port_option(m)?
        .filter(|port| *port != 0)
        .unwrap_or(settings.port);
    Ok((host, port))
}

fn probe_for(base: &str) -> Result<wg_uispec::Probe, CliFailure> {
    wg_uispec::probe_of(&wg_uispec::health_url(base))
        .map_err(|message| CliFailure::fail(message, 1))
}

fn log_path(state: &CliState) -> Result<PathBuf, CliFailure> {
    let paths = resolve_paths(state.home.as_deref())?;
    Ok(PathBuf::from(paths.log_dir().as_str()).join(wg_uispec::SERVE_LOG))
}

/// Takes the exclusive lock that serialises `serve --ensure` invocations. The returned handle holds
/// the lock until it is dropped.
fn acquire_ensure_lock(state: &CliState) -> Result<std::fs::File, CliFailure> {
    let paths = resolve_paths(state.home.as_deref())?;
    let dir = PathBuf::from(paths.data_dir().as_str());
    std::fs::create_dir_all(&dir)
        .map_err(|err| CliFailure::fail(format!("cannot create {}: {err}", dir.display()), 1))?;
    let path = dir.join(ENSURE_LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|err| CliFailure::fail(format!("cannot open {}: {err}", path.display()), 1))?;
    file.lock()
        .map_err(|err| CliFailure::fail(format!("cannot lock {}: {err}", path.display()), 1))?;
    Ok(file)
}

fn stop_path(state: &CliState) -> Result<PathBuf, CliFailure> {
    let paths = resolve_paths(state.home.as_deref())?;
    Ok(PathBuf::from(paths.data_dir().as_str()).join(STOP_FILE))
}

/// What a daemon command reports: the status word, the base URL, and the process id when this
/// call started the server.
struct Outcome {
    status: &'static str,
    base: String,
    pid: Option<u32>,
    version: Option<String>,
}

impl Outcome {
    fn value(&self) -> Value {
        dict!({
            "status": Value::Str(PyStr::from(self.status)),
            "base": Value::Str(PyStr::from(self.base.as_str())),
            "pid": self.pid.map_or(Value::Null, |pid| Value::Int(i128::from(pid))),
            "version": self.version.as_deref().map_or(Value::Null, |v| Value::Str(PyStr::from(v))),
        })
    }

    fn human(&self) -> String {
        match self.pid {
            Some(pid) => format!(
                "{}: workgraph serve at {}, pid {pid}, version {}\n",
                self.status,
                self.base,
                self.version.as_deref().unwrap_or("unknown")
            ),
            None => format!("{}: workgraph serve at {}\n", self.status, self.base),
        }
    }
}

/// `serve --ensure`: the running daemon's status, or the one this call started. `--passive` is the
/// form the Claude Code hooks run: it never prints and never fails, so it always exits 0.
pub fn ensure(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let passive = option_flag(m, "--passive");
    let outcome = ensure_outcome(state, m);
    if passive {
        return Ok(());
    }
    let outcome = outcome?;
    let human = outcome.human();
    state.emit(&outcome.value(), Some(&|_| human.clone()))
}

fn ensure_outcome(state: &mut CliState, m: &ArgMatches) -> Result<Outcome, CliFailure> {
    let (host, port) = daemon_listen(state, m)?;
    let base = base_url(None, &host, port);
    let probe = probe_for(&base)?;
    // The lock covers the probe, the start and the wait for the started server to answer. A racing
    // invocation blocks here, then finds the server answering and starts nothing. The lock is an
    // open file handle, so the operating system releases it when this call ends or dies.
    let _lock = acquire_ensure_lock(state)?;
    if answering(&probe) {
        return Ok(Outcome {
            status: "running",
            base,
            pid: None,
            version: None,
        });
    }
    let log = log_path(state)?;
    let home = state.home.clone();
    match start_and_wait(
        home.as_deref(),
        Some((&host, port)),
        &log,
        &probe,
        &base,
        START_TIMEOUT,
    ) {
        Ok(pid) => Ok(Outcome {
            status: "started",
            version: health_version(&base),
            base,
            pid: Some(pid),
        }),
        // Another invocation started the server while this one did: the port is answered by it.
        Err(err) => {
            if answering_within(&probe, START_TIMEOUT) {
                Ok(Outcome {
                    status: "running",
                    base,
                    pid: None,
                    version: None,
                })
            } else {
                Err(err)
            }
        }
    }
}

/// Probes until the server answers or `seconds` pass.
fn answering_within(probe: &wg_uispec::Probe, seconds: f64) -> bool {
    let started = Instant::now();
    loop {
        if answering(probe) {
            return true;
        }
        if started.elapsed().as_secs_f64() >= seconds {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// `serve --restart`: stops the running daemon through its stop file, then starts one from this
/// binary. Reports the new process id and the version the health endpoint answers.
pub fn restart(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let (host, port) = daemon_listen(state, m)?;
    let base = base_url(None, &host, port);
    let probe = probe_for(&base)?;
    let stop = stop_path(state)?;
    if answering(&probe) {
        std::fs::write(&stop, "stop\n")
            .map_err(|err| CliFailure::fail(format!("cannot ask the daemon to stop: {err}"), 1))?;
        let started = Instant::now();
        while answering(&probe) {
            if started.elapsed().as_secs_f64() >= STOP_TIMEOUT {
                return Err(CliFailure::fail(
                    format!(
                        "the daemon at {base} did not stop within {STOP_TIMEOUT} seconds; it may predate `serve --restart`, so stop it by hand and run this again"
                    ),
                    1,
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    // The old daemon removes the stop file itself; a leftover one would stop the next daemon.
    let _ = std::fs::remove_file(&stop);
    let log = log_path(state)?;
    let home = state.home.clone();
    let pid = start_and_wait(
        home.as_deref(),
        Some((&host, port)),
        &log,
        &probe,
        &base,
        START_TIMEOUT,
    )?;
    let outcome = Outcome {
        status: "restarted",
        base: base.clone(),
        pid: Some(pid),
        version: health_version(&base),
    };
    let human = outcome.human();
    state.emit(&outcome.value(), Some(&|_| human.clone()))
}

/// The `version` the health endpoint answers, when it answers.
fn health_version(base: &str) -> Option<String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent.get(&wg_uispec::health_url(base)).call().ok()?;
    let text = response.body_mut().read_to_string().ok()?;
    let body: serde_json::Value = serde_json::from_str(&text).ok()?;
    body.get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Polls for the stop file on a thread and cancels `stop` when it appears, removing the file. The
/// thread ends once `stop` is cancelled, by any cause.
pub fn watch_stop_file(path: PathBuf, stop: CancellationToken) {
    let _ = std::thread::Builder::new()
        .name("workgraph-stop-watch".to_owned())
        .spawn(move || {
            loop {
                if stop.is_cancelled() {
                    return;
                }
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                    stop.cancel();
                    return;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        });
}
