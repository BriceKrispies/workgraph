//! wg-browser: locating Chromium-family browsers and CDP screenshots.
//!
//! The shell half. The search order, the options and every DevTools message are pure rules in
//! `wg-browserspec`; this crate reads the environment, starts the browser, opens the sockets and
//! waits. No browser is bundled: Chrome, Edge or Chromium is driven over the Chrome DevTools
//! Protocol.

mod cdp;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cdp::Cdp;
use wg_browserspec::{
    capture_calls, capture_screenshot_call, devtools_port, io_failure, launch_args, locate_browser,
    page_websocket_url, readiness_expression, screenshot_bytes, unavailable, wait_timeout_error,
};
use wg_types::WgError;

pub use wg_browserspec::{BROWSER_ENV, CaptureOptions};

const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
const READY_POLL: Duration = Duration::from_millis(200);

/// A written screenshot and the settings it was taken with.
#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    pub url: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub browser: PathBuf,
}

/// The browser to drive: `explicit`, then `WORKGRAPH_BROWSER`, then `PATH` and the install
/// locations.
///
/// # Errors
/// A `ProviderUnavailable` error when the named browser does not exist or none is installed.
pub fn find_browser(explicit: Option<&str>) -> Result<PathBuf, WgError> {
    locate_browser(explicit, &|name| std::env::var(name).ok(), &|path| {
        path.is_file()
    })
}

/// Loads `url` in a headless browser and writes a PNG of the viewport to `out`.
///
/// The options are validated before any browser is looked for. The capture waits for the page to
/// load, for `wait_for` to match and `wait_gone` to stop matching, then for `settle` seconds.
///
/// # Errors
/// A `ValidationFailed` error for bad options; a `ProviderUnavailable` error when no browser is
/// found, it fails to start, or the page does not become ready in time.
pub fn screenshot(
    url: &str,
    out: &Path,
    options: &CaptureOptions,
    browser: Option<&str>,
) -> Result<Capture, WgError> {
    options.validate()?;
    let exe = find_browser(browser)?;
    let out = std::path::absolute(out).map_err(|err| io_failure("resolve output path", err))?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| io_failure("create output directory", err))?;
    }
    let profile = ProfileDir::create()?;
    let mut child = Command::new(&exe)
        .args(launch_args(
            &profile.0.to_string_lossy(),
            options.width,
            options.height,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| io_failure("start the browser", err))?;
    let result = drive(&mut child, &profile.0, url, &out, options);
    // Chrome is terminated whether or not the capture worked.
    let _ = child.kill();
    let _ = child.wait();
    result?;
    Ok(Capture {
        url: url.to_owned(),
        path: out,
        width: options.width,
        height: options.height,
        scale: options.scale,
        browser: exe,
    })
}

fn drive(
    child: &mut Child,
    profile: &Path,
    url: &str,
    out: &Path,
    options: &CaptureOptions,
) -> Result<(), WgError> {
    let port = wait_for_devtools_port(child, profile)?;
    let ws_url = wait_for_page_socket(port)?;
    let mut cdp = Cdp::connect(port, &ws_url, START_TIMEOUT)?;
    let timeout = Duration::from_secs_f64(options.timeout.max(0.0));
    for call in capture_calls(options, url) {
        cdp.call(&call, START_TIMEOUT)?;
    }
    let ready = readiness_expression(options.wait_for.as_deref(), options.wait_gone.as_deref());
    let deadline = Instant::now() + timeout;
    while !cdp.evaluate_true(&ready, START_TIMEOUT)? {
        if Instant::now() > deadline {
            return Err(wait_timeout_error(
                options.timeout,
                url,
                options.wait_for.as_deref(),
                options.wait_gone.as_deref(),
            ));
        }
        std::thread::sleep(READY_POLL);
    }
    std::thread::sleep(Duration::from_secs_f64(options.settle.max(0.0)));
    let shot = cdp.call(&capture_screenshot_call(), timeout)?;
    let bytes = screenshot_bytes(&shot)?;
    std::fs::write(out, bytes).map_err(|err| io_failure("write the screenshot", err))
}

fn wait_for_devtools_port(child: &mut Child, profile: &Path) -> Result<u16, WgError> {
    let marker = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| io_failure("poll the browser", err))?
        {
            let code = status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
            return Err(unavailable(format!(
                "browser exited during start-up (code {code})"
            )));
        }
        if let Some(port) = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|text| devtools_port(&text))
        {
            return Ok(port);
        }
        std::thread::sleep(POLL);
    }
    Err(unavailable("browser did not open a DevTools port"))
}

fn wait_for_page_socket(port: u16) -> Result<String, WgError> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(url) = fetch_targets(port).as_deref().and_then(page_websocket_url) {
            return Ok(url);
        }
        std::thread::sleep(POLL);
    }
    Err(unavailable("browser opened no page target"))
}

fn fetch_targets(port: u16) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();
    agent
        .get(&url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()
}

/// A throwaway Chrome profile directory, removed when the capture ends.
struct ProfileDir(PathBuf);

impl ProfileDir {
    fn create() -> Result<Self, WgError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path =
            std::env::temp_dir().join(format!("workgraph-browser-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)
            .map_err(|err| io_failure("create the browser profile", err))?;
        Ok(Self(path))
    }
}

impl Drop for ProfileDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
