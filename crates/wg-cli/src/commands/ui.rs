//! `workgraph ui open [PAGE] [--url] [--no-start] [--print] [--timeout]` (C192) and
//! `workgraph ui screenshot` (IF-23).
//!
//! The decisions, URLs and messages are `wg_uispec`; this module does the I/O: the configuration
//! read, the health probe, starting `workgraph serve` detached and polling it, and launching the
//! browser.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::spec::ui::{
    SCREENSHOT, Shot, base_url, health_url, no_server_message, number, target_url, wait_selector,
};
use clap::ArgMatches;
use wg_browser::{CaptureOptions, screenshot};
use wg_iface::cli::args::{option_flag, option_text};
use wg_iface::cli::{CliFailure, not_implemented_message};
use wg_iface::http::serve::Settings;
use wg_uispec::{Platform, Start};

use super::Group;
use super::daemon::{answering, start_and_wait};
use crate::CliState;
use wg_proc::Spawn;

/// The `ui` group.
pub const GROUP: Group = Group {
    name: "ui",
    command: wg_uispec::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("open") => open(state, m),
        Some("screenshot") => capture(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

fn open(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let request = wg_uispec::open_request(m)?;
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
                start_and_wait(home, None, &log, &probe, &base, request.timeout)?;
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
    let _ = Spawn::new(program)
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
    // Click checks the numeric options before the command runs: a bad one fails before any
    // server is probed or any config is read.
    let defaults = CaptureOptions::default();
    let width = number(m, "--width", defaults.width, SCREENSHOT, "int")?;
    let height = number(m, "--height", defaults.height, SCREENSHOT, "int")?;
    let scale = number(m, "--scale", defaults.scale, SCREENSHOT, "float")?;
    let settle = number(m, "--settle", defaults.settle, SCREENSHOT, "float")?;
    let timeout = number(m, "--timeout", defaults.timeout, SCREENSHOT, "float")?;
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
    let options = CaptureOptions {
        width,
        height,
        scale,
        wait_for: Some(wait_selector(
            &page,
            option_text(m, "--wait-for").as_deref(),
        )),
        settle,
        timeout,
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
