//! `workgraph ui open [PAGE] [--url] [--no-start] [--print] [--timeout]` (C192) and
//! `workgraph ui screenshot` (IF-23).
//!
//! The decisions, URLs and messages are `wg_uispec`; this module does the I/O: the configuration
//! read, the health probe, starting `workgraph serve` detached and polling it, and launching the
//! browser.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use clap::ArgMatches;
use wg_browser::{CaptureOptions, screenshot};
use wg_clispec::task::{option_flag, option_text};
use wg_clispec::ui::{
    Shot, base_url, health_url, no_server_message, number, target_url, wait_selector,
};
use wg_clispec::{CliFailure, not_implemented_message};
use wg_serverspec::serve::Settings;
use wg_uispec::{Platform, Start, Wait};

use super::Group;
use crate::CliState;

/// The `ui` group.
pub const GROUP: Group = Group {
    name: "ui",
    command: wg_uispec::command,
    run,
};

/// Exit code of a usage error (the one clap uses).
const EXIT_USAGE: u8 = 2;

/// How often a started server is probed while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("open") => open(state, m),
        Some("screenshot") => capture(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

fn open(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let request =
        wg_uispec::open_request(m).map_err(|message| CliFailure::fail(message, EXIT_USAGE))?;
    let base = match request.url.as_deref() {
        Some(url) if !url.is_empty() => wg_uispec::normalize_base(url),
        _ => {
            let config = state.app()?.config()?;
            wg_uispec::base_from_config(&config).map_err(|message| CliFailure::fail(message, 1))?
        }
    };
    let target = wg_uispec::target_url(&base, &request.page);
    let probe = wg_uispec::probe_of(&wg_uispec::health_url(&base))
        .map_err(|message| CliFailure::fail(message, 1))?;
    let started =
        match wg_uispec::start_decision(answering(&probe), request.url.is_some(), request.no_start)
        {
            Start::UseRunning => false,
            Start::Refuse => return Err(CliFailure::fail(wg_uispec::no_server_message(&base), 1)),
            Start::Launch => {
                let log =
                    PathBuf::from(state.app()?.paths.log_dir().as_str()).join(wg_uispec::SERVE_LOG);
                let home = state.home.as_deref();
                start_and_wait(home, &log, &probe, &base, request.timeout)?;
                true
            }
        };
    if !request.print {
        open_browser(&target);
    }
    let value = wg_uispec::opened_value(&target, started);
    let human = wg_uispec::human(&target, &base, started, request.print);
    state.emit(&value, Some(&|_| human.clone()))
}

/// One health probe: true when the server answers with a success status.
fn answering(probe: &wg_uispec::Probe) -> bool {
    wg_net::http_status_line(
        &probe.authority,
        &wg_uispec::request_text(probe),
        Duration::from_secs_f64(wg_uispec::PROBE_TIMEOUT),
    )
    .is_ok_and(|line| wg_uispec::answers_line(&line))
}

/// Starts `workgraph serve` detached and polls it until it answers, the process ends or the
/// timeout passes.
fn start_and_wait(
    home: Option<&Path>,
    log: &Path,
    probe: &wg_uispec::Probe,
    base: &str,
    timeout: f64,
) -> Result<(), CliFailure> {
    let log_text = log.display().to_string();
    let mut child = spawn_serve(home, log)
        .map_err(|err| CliFailure::fail(format!("cannot start `workgraph serve`: {err}"), 1))?;
    let started_at = Instant::now();
    loop {
        let up = answering(probe);
        let exited = child
            .try_wait()
            .map_err(|err| CliFailure::fail(format!("cannot watch `workgraph serve`: {err}"), 1))?
            .map(|status| status.code().unwrap_or(1));
        let elapsed = started_at.elapsed().as_secs_f64();
        match wg_uispec::wait_step(up, exited, elapsed, timeout) {
            Wait::Ready => return Ok(()),
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

/// Launches this binary's `serve` with its output appended to `log`. On Windows the server is
/// detached from the console; on POSIX it gets its own process group.
fn spawn_serve(home: Option<&Path>, log: &Path) -> std::io::Result<Child> {
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
    let mut command = Command::new(exe);
    command
        .args(wg_uispec::serve_args(home_text.as_deref()))
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(wg_uispec::WINDOWS_DETACHED);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

/// Opens `url` in the default browser. Like `webbrowser.open`, a browser that cannot be started
/// is not an error of the command.
fn open_browser(url: &str) {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Other
    };
    let (program, args) = wg_uispec::browser_command(platform, url);
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Asks the health endpoint, as `httpx.get(..., timeout=5).raise_for_status()`.
fn health(url: &str) -> Result<(), String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();
    agent
        .get(url)
        .call()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn capture(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let settings = Settings::from_config(&state.app()?.config()?);
    let base = base_url(
        option_text(m, "--url").as_deref(),
        &settings.host,
        settings.port,
    );
    health(&health_url(&base))
        .map_err(|reason| CliFailure::fail(no_server_message(&base, &reason), 1))?;
    let page = option_text(m, "page").unwrap_or_else(|| "/".to_owned());
    let target = target_url(&base, &page);
    let defaults = CaptureOptions::default();
    let options = CaptureOptions {
        width: number(m, "--width", defaults.width)?,
        height: number(m, "--height", defaults.height)?,
        scale: number(m, "--scale", defaults.scale)?,
        wait_for: Some(wait_selector(
            &page,
            option_text(m, "--wait-for").as_deref(),
        )),
        settle: number(m, "--settle", defaults.settle)?,
        timeout: number(m, "--timeout", defaults.timeout)?,
        theme: option_text(m, "--theme"),
        mobile: option_flag(m, "--mobile"),
        ..defaults
    };
    let out = PathBuf::from(
        option_text(m, "--out").unwrap_or_else(|| "workgraph-screenshot.png".to_owned()),
    );
    let done = screenshot(
        &target,
        &out,
        &options,
        option_text(m, "--browser").as_deref(),
    )?;
    let shot = Shot {
        url: done.url,
        file: done.path.display().to_string(),
        width: done.width,
        height: done.height,
        scale: done.scale,
        browser: done.browser.display().to_string(),
        theme: options.theme,
        mobile: options.mobile,
    };
    let pairs = shot.pairs();
    state.emit(
        &shot.value(),
        Some(&|console| {
            let shown: Vec<(&str, Option<&str>)> = pairs
                .iter()
                .map(|(key, value)| (*key, Some(value.as_str())))
                .collect();
            console.kv(&shown, None).text
        }),
    )
}
