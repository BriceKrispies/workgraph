//! `workgraph ui screenshot` (IF-23, `cli/commands/ui.py`). `ui open` stays a placeholder until
//! its own task.

use std::path::PathBuf;
use std::time::Duration;

use clap::ArgMatches;
use wg_browser::{CaptureOptions, screenshot};
use wg_clispec::task::{option_flag, option_text};
use wg_clispec::ui::{
    self as view, Shot, base_url, health_url, no_server_message, number, target_url, wait_selector,
};
use wg_clispec::{CliFailure, not_implemented_message};
use wg_serverspec::serve::Settings;

use super::Group;
use crate::CliState;

/// The `ui` group.
pub const GROUP: Group = Group {
    name: "ui",
    command: view::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("screenshot") => capture(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
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
