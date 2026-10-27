//! wg-uispec: the pure half of `workgraph ui open` (contract C192).
//!
//! The clap command of the `ui` group, the URL of the server and of a page, the health probe's
//! request and the reading of its status line, the decision whether to start `workgraph serve`,
//! the steps of waiting for a started server, the browser command of each platform and the
//! messages. Nothing here starts a process, opens a socket, reads the environment or the clock:
//! the shell (`wg-cli`, `wg-net`) passes those values in.

use clap::{ArgMatches, Command};
use serde_json::{Map, Value as Json};
use wg_clispec::failure::python_number_text;
use wg_clispec::tree::{SNAPSHOT_JSON, command as bare_command, placeholder};
use wg_clispec::{CliFailure, Snapshot, Usage};
use wg_json::{PyStr, Value, dict};

/// The health endpoint of a Workgraph server, relative to its base URL.
pub const HEALTH_PATH: &str = "api/v1/health";

/// `--timeout` when it is not given: seconds to wait for a server this command started.
pub const DEFAULT_TIMEOUT: f64 = 20.0;

/// Seconds one health probe may take to connect and to answer.
pub const PROBE_TIMEOUT: f64 = 1.0;

/// Log file of a server started by this command, under the log directory.
pub const SERVE_LOG: &str = "serve-detached.log";

/// `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`: the flags that detach a started server from the
/// console on Windows.
pub const WINDOWS_DETACHED: u32 = 0x0000_0008 | 0x0000_0200;

/// The clap command of the `ui` group, built from the snapshot (`screenshot` stays a placeholder
/// until its task lands).
#[must_use]
pub fn command() -> Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "ui"),
        Err(_) => bare_command("ui"),
    }
}

/// The parsed arguments of `ui open`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRequest {
    /// The UI path, possibly empty (the home page).
    pub page: String,
    /// `--url`, as given (an empty value still counts as given, as in Python).
    pub url: Option<String>,
    pub no_start: bool,
    pub print: bool,
    /// Seconds to wait for a started server.
    pub timeout: f64,
}

/// The usage of `ui open`, as Click prints it in a usage error.
const OPEN: Usage = Usage {
    usage: "workgraph ui open [OPTIONS] [page]",
    command: "workgraph ui open",
};

/// Reads the `ui open` matches. A `--timeout` that is not a finite float is a Click usage error
/// (exit 2). Python also takes `nan` and `inf`; a non-finite wait cannot be a `Duration`, so
/// those stay a usage error here.
///
/// # Errors
/// A usage failure for a `--timeout` that is not a finite number.
pub fn open_request(m: &ArgMatches) -> Result<OpenRequest, CliFailure> {
    let timeout = match text(m, "--timeout") {
        None => DEFAULT_TIMEOUT,
        Some(raw) => python_number_text(&raw)
            .and_then(|digits| digits.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .ok_or_else(|| OPEN.invalid_value("--timeout", &raw, "float"))?,
    };
    Ok(OpenRequest {
        page: text(m, "page").unwrap_or_default(),
        url: text(m, "--url"),
        no_start: flag(m, "--no-start"),
        print: flag(m, "--print"),
        timeout,
    })
}

fn text(m: &ArgMatches, id: &str) -> Option<String> {
    m.try_get_one::<String>(id).ok().flatten().cloned()
}

fn flag(m: &ArgMatches, id: &str) -> bool {
    m.try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

/// `base_url`: the given URL with exactly one trailing slash.
#[must_use]
pub fn normalize_base(url: &str) -> String {
    format!("{}/", url.trim_end_matches('/'))
}

/// The base URL the configuration names (`http://host:port/`). A missing `[server]` section or
/// key is an error.
pub fn base_from_config(config: &Map<String, Json>) -> Result<String, String> {
    let server = config
        .get("server")
        .and_then(Json::as_object)
        .ok_or_else(|| "the configuration has no [server] section".to_owned())?;
    let host = server
        .get("host")
        .and_then(Json::as_str)
        .ok_or_else(|| "the configuration has no server.host".to_owned())?;
    let port = match server.get("port") {
        Some(Json::Number(number)) => number.to_string(),
        Some(Json::String(text)) => text.clone(),
        _ => return Err("the configuration has no server.port".to_owned()),
    };
    Ok(format!("http://{host}:{port}/"))
}

/// The URL of a UI page under `base` (`urljoin(base, page.lstrip("/"))`).
#[must_use]
pub fn target_url(base: &str, page: &str) -> String {
    format!("{base}{}", page.trim_start_matches('/'))
}

/// The health URL under `base`.
#[must_use]
pub fn health_url(base: &str) -> String {
    format!("{base}{HEALTH_PATH}")
}

/// What the health probe connects to: the `host:port` authority and the request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub authority: String,
    pub path: String,
}

/// The probe of a plain `http://` URL. Other schemes cannot be probed by this build.
pub fn probe_of(url: &str) -> Result<Probe, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs can be checked, not {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("no host in {url}"));
    }
    Ok(Probe {
        authority: authority.to_owned(),
        path: path.to_owned(),
    })
}

/// The text of the probe's GET request.
#[must_use]
pub fn request_text(probe: &Probe) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        probe.path, probe.authority
    )
}

/// The status code of an HTTP status line (`HTTP/1.1 200 OK`).
#[must_use]
pub fn status_code(status_line: &str) -> Option<u16> {
    let mut parts = status_line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// True when the status line is a success (2xx), the bar `raise_for_status` sets.
#[must_use]
pub fn answers_line(status_line: &str) -> bool {
    status_code(status_line).is_some_and(|code| (200..300).contains(&code))
}

/// What to do before opening the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// A server answers: use it.
    UseRunning,
    /// Nothing answers and one may be started.
    Launch,
    /// Nothing answers and none may be started (`--url` or `--no-start`).
    Refuse,
}

/// The start decision: a running server is always used; without one, a server is started unless
/// the caller gave `--url` (a server it names is never started) or `--no-start`.
#[must_use]
pub fn start_decision(answering: bool, url_given: bool, no_start: bool) -> Start {
    if answering {
        Start::UseRunning
    } else if url_given || no_start {
        Start::Refuse
    } else {
        Start::Launch
    }
}

/// One step of waiting for a started server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    Ready,
    Keep,
    /// The server process ended with this exit code before it answered.
    Exited(i32),
    TimedOut,
}

/// The step after one probe: an answer wins, then a dead process, then the deadline. `exited` is
/// the exit code of the started process if it has ended.
#[must_use]
pub fn wait_step(answering: bool, exited: Option<i32>, elapsed: f64, timeout: f64) -> Wait {
    if answering {
        Wait::Ready
    } else if let Some(code) = exited {
        Wait::Exited(code)
    } else if elapsed >= timeout {
        Wait::TimedOut
    } else {
        Wait::Keep
    }
}

/// The arguments that start the server: `[--home H] serve`.
#[must_use]
pub fn serve_args(home: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(home) = home {
        args.push("--home".to_owned());
        args.push(home.to_owned());
    }
    args.push("serve".to_owned());
    args
}

/// The platforms whose default-browser command differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Other,
}

/// The program and arguments that open `url` in the default browser. The URL is one argument,
/// never a shell command line.
#[must_use]
pub fn browser_command(platform: Platform, url: &str) -> (&'static str, Vec<String>) {
    let program = match platform {
        Platform::Windows => "explorer",
        Platform::MacOs => "open",
        Platform::Other => "xdg-open",
    };
    (program, vec![url.to_owned()])
}

/// The message when nothing answers and no server may be started.
#[must_use]
pub fn no_server_message(base: &str) -> String {
    format!("no Workgraph server at {base}; start one with `workgraph serve`")
}

/// The message when the started server process ended before it answered.
#[must_use]
pub fn exited_message(code: i32, log: &str) -> String {
    format!("`workgraph serve` exited with code {code}; see {log}")
}

/// The message when the started server did not answer within the timeout.
#[must_use]
pub fn timed_out_message(base: &str, timeout: f64, log: &str) -> String {
    format!("`workgraph serve` did not answer at {base} within {timeout}s; see {log}")
}

/// The result: `{"url", "started"}`.
#[must_use]
pub fn opened_value(url: &str, started: bool) -> Value {
    dict!({
        "url": Value::Str(PyStr::from(url)),
        "started": Value::Bool(started),
    })
}

/// The human output: the URL alone with `--print`, else the start line (when started) and the
/// opened URL.
#[must_use]
pub fn human(url: &str, base: &str, started: bool, print: bool) -> String {
    if print {
        return format!("{url}\n");
    }
    let mut text = String::new();
    if started {
        text.push_str(&format!("started server at {base}\n"));
    }
    text.push_str(&format!("opened {url}\n"));
    text
}
