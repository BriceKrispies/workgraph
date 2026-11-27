//! Shell: the Claude Code commands' services (`cli/commands/claude.py` and the installer calls of
//! `integrations/claude_code/installer.py`). The settings file, the environment, the executable
//! search and the hook ingestion are read here; the decisions are [`crate::spec::installer`].
//!
//! The services take the [`ClaudeEnv`] they read the home directory and `CLAUDE_CONFIG_DIR`
//! from, so tests run them against a temporary home and never touch the user's settings.

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use wg_proc::{CaptureLimits, run_capture};
use wg_procspec::ProcessSpec;
use wg_runtime::{App, human_context, write_atomic};
use wg_types::{WgError, parse_id};

use crate::service::hooks;
use crate::spec::installer::{self, InstallerStatus};

/// The environment the Claude Code commands read: the home directory (`Path.home()`) and
/// `CLAUDE_CONFIG_DIR` when it is set and not empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeEnv {
    pub home: PathBuf,
    pub config_dir: Option<PathBuf>,
}

impl ClaudeEnv {
    /// The process environment. `Path.home()` reads `USERPROFILE` on Windows and `HOME` elsewhere.
    #[must_use]
    pub fn from_process() -> Self {
        let home_variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(home_variable)
            .map(PathBuf::from)
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self { home, config_dir }
    }
}

/// `installer.status(scope, project_dir)`: reads the settings file of `scope` and reports the
/// Workgraph handlers in it.
#[must_use]
pub fn installation(env: &ClaudeEnv, scope: &str, project_dir: Option<&Path>) -> InstallerStatus {
    let root = project_root(project_dir);
    let path = installer::settings_path(scope, env.config_dir.as_deref(), &env.home, &root);
    let shown = path.display().to_string();
    let exists = path.exists();
    let loaded = load(&path, &shown);
    installer::status(scope, &shown, exists, loaded, command_resolves)
}

/// `_load(path)`: the settings object of a file, `{}` when the file is absent or empty, or the
/// refusal text when it cannot be used.
fn load(path: &Path, shown: &str) -> Result<serde_json::Map<String, Json>, String> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{shown} could not be read: {err}"))?;
    installer::parse_settings(shown, &text)
}

/// `Path(project_dir or Path.cwd()).resolve()`.
fn project_root(project_dir: Option<&Path>) -> PathBuf {
    let base = project_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    PathBuf::from(hooks::resolved_path(&base.to_string_lossy()))
}

/// `Path(c).exists() or shutil.which(c)`.
fn command_resolves(command: &str) -> bool {
    Path::new(command).exists() || find_on_path(command).is_some()
}

/// `shutil.which(name)`: the first file on `PATH` called `name` (with each `PATHEXT` extension on
/// Windows).
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = executable_names(name);
    std::env::split_paths(&path)
        .flat_map(|dir| {
            names
                .iter()
                .map(|file| dir.join(file))
                .collect::<Vec<PathBuf>>()
        })
        .find(|candidate| candidate.is_file())
}

/// The file names `name` may have: itself, or `name` plus each `PATHEXT` extension on Windows.
fn executable_names(name: &str) -> Vec<String> {
    if !cfg!(windows) {
        return vec![name.to_owned()];
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    extensions
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(|ext| format!("{name}{ext}"))
        .collect()
}

/// `workgraph_executable()`: the `workgraph` on `PATH`, resolved, else the one beside this
/// executable.
pub fn workgraph_executable() -> Result<String, WgError> {
    if let Some(found) = find_on_path("workgraph") {
        return Ok(hooks::resolved_path(&found.to_string_lossy()));
    }
    let file = if cfg!(windows) {
        "workgraph.exe"
    } else {
        "workgraph"
    };
    std::env::current_exe()
        .ok()
        .and_then(|me| me.parent().map(|dir| dir.join(file)))
        .filter(|candidate| candidate.exists())
        .map(|candidate| hooks::resolved_path(&candidate.to_string_lossy()))
        .ok_or_else(|| {
            WgError::config(
                "could not locate the workgraph executable; install it or pass --command",
            )
        })
}

/// `mcp-config`: the MCP client block for `command`, or for the located executable.
pub fn mcp_config(command: Option<&str>) -> Result<Json, WgError> {
    let exe = match command.filter(|text| !text.is_empty()) {
        Some(text) => text.to_owned(),
        None => workgraph_executable()?,
    };
    Ok(installer::mcp_server_config(&exe))
}

/// `installer.install(scope, project_dir=..., events=..., command=...)` and, with `--mcp`, the
/// `claude mcp add` registration. `events` empty means every default event; `command` empty or
/// absent locates the executable. The result is the Python dict, keys in its order.
pub fn install(
    env: &ClaudeEnv,
    scope: &str,
    project_dir: Option<&Path>,
    events: &[String],
    command: Option<&str>,
    mcp: bool,
    http: Option<&McpHttp>,
) -> Result<Json, WgError> {
    if mcp && http.is_some() {
        return Err(WgError::validation(
            "--mcp and --mcp-http each write the Claude Code registration; choose one",
        ));
    }
    let root = project_root(project_dir);
    let path = installer::settings_path(scope, env.config_dir.as_deref(), &env.home, &root);
    let shown = path.display().to_string();
    let data = load(&path, &shown).map_err(installer::config_error)?;
    let exe = match command.filter(|text| !text.is_empty()) {
        Some(text) => text.to_owned(),
        None => workgraph_executable()?,
    };
    let events: Vec<String> = if events.is_empty() {
        installer::DEFAULT_EVENTS
            .iter()
            .map(|event| (*event).to_owned())
            .collect()
    } else {
        events.to_vec()
    };
    let (data, added) =
        installer::install_hooks(data, &shown, &events, &exe).map_err(installer::config_error)?;
    let (data, ensure_added) =
        installer::install_ensure(data, &shown, &exe).map_err(installer::config_error)?;
    let backup = if added.is_empty() && !ensure_added {
        None
    } else {
        write_settings(&path, &data)?
    };
    let mut out = installer::install_object(&shown, &added, backup.as_deref(), &exe);
    out.insert("ensure".to_owned(), Json::Bool(ensure_added));
    out.insert(
        "autostart".to_owned(),
        Json::String(install_autostart(&exe)?),
    );
    if mcp {
        let registration = register_mcp(scope, &exe)?;
        out.insert("mcp".to_owned(), Json::String(registration));
    }
    if let Some(http) = http {
        let registration = register_mcp_http(scope, http)?;
        out.insert("mcp_http".to_owned(), Json::String(registration));
    }
    Ok(Json::Object(out))
}

/// `installer.uninstall(scope, project_dir=...)`: removes only the Workgraph handlers (the hooks
/// and the daemon's ensure handler) and the logon autostart.
pub fn uninstall(
    env: &ClaudeEnv,
    scope: &str,
    project_dir: Option<&Path>,
) -> Result<Json, WgError> {
    let root = project_root(project_dir);
    let path = installer::settings_path(scope, env.config_dir.as_deref(), &env.home, &root);
    let shown = path.display().to_string();
    let data = load(&path, &shown).map_err(installer::config_error)?;
    let (data, removed) = installer::uninstall_hooks(data);
    let backup = if removed.is_empty() {
        None
    } else {
        write_settings(&path, &data)?
    };
    let mut out = installer::uninstall_object(&shown, &removed, backup.as_deref());
    out.insert("autostart".to_owned(), Json::String(remove_autostart()?));
    Ok(Json::Object(out))
}

/// The HTTP registration: the endpoint and the bearer token the daemon answers with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpHttp {
    pub url: String,
    pub token: String,
}

/// The per-user logon autostart file (Windows: the Startup folder). None elsewhere.
fn autostart_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    std::env::var_os("APPDATA").map(|appdata| {
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join(AUTOSTART_FILE)
    })
}

/// The Startup-folder file's name.
const AUTOSTART_FILE: &str = "workgraph-serve.cmd";

/// Writes the logon autostart that starts the daemon with `exe`. Reports where it was written, or
/// why it was not.
fn install_autostart(exe: &str) -> Result<String, WgError> {
    let Some(path) = autostart_path() else {
        return Ok("not available: no per-user Startup folder on this platform".to_owned());
    };
    std::fs::write(&path, installer::autostart_text(exe))
        .map_err(|err| WgError::workgraph(format!("could not write {}: {err}", path.display())))?;
    Ok(format!("installed {}", path.display()))
}

/// Removes the logon autostart file, when it exists.
fn remove_autostart() -> Result<String, WgError> {
    let Some(path) = autostart_path() else {
        return Ok("not available: no per-user Startup folder on this platform".to_owned());
    };
    if !path.exists() {
        return Ok("absent".to_owned());
    }
    std::fs::remove_file(&path)
        .map_err(|err| WgError::workgraph(format!("could not remove {}: {err}", path.display())))?;
    Ok(format!("removed {}", path.display()))
}

/// `--mcp-http`: replaces the user-scope `workgraph` registration with the HTTP endpoint of the
/// daemon, by `claude mcp remove` then `claude mcp add --transport http` (WG-5702). Returns the
/// output of the add.
fn register_mcp_http(scope: &str, http: &McpHttp) -> Result<String, WgError> {
    let Some(claude) = find_on_path("claude") else {
        return Err(WgError::workgraph(
            "claude CLI not found on the search path; register the HTTP endpoint by hand",
        ));
    };
    let claude = claude.to_string_lossy().into_owned();
    // The old registration may be absent; a failed removal is not an error of the replacement.
    let mut removal = ProcessSpec::new(installer::mcp_remove_argv(&claude, scope));
    removal.timeout_ms = Some(MCP_ADD_TIMEOUT_MS);
    let _ = run_capture(&removal, CaptureLimits::default(), None);
    let argv = installer::mcp_add_http_argv(&claude, scope, &http.url, &http.token, HTTP_AGENT);
    let mut spec = ProcessSpec::new(argv);
    spec.timeout_ms = Some(MCP_ADD_TIMEOUT_MS);
    let result = run_capture(&spec, CaptureLimits::default(), None)
        .map_err(|err| WgError::workgraph(err.to_string()))?;
    if result.timed_out {
        return Err(WgError::workgraph(format!(
            "claude mcp add timed out after {} seconds",
            MCP_ADD_TIMEOUT_MS / 1000
        )));
    }
    if result.exit_code != Some(0) {
        return Err(WgError::workgraph(format!(
            "claude mcp add failed: {}",
            installer::mcp_output(
                &String::from_utf8_lossy(&result.stdout),
                &String::from_utf8_lossy(&result.stderr),
            )
        )));
    }
    Ok(installer::mcp_output(
        &String::from_utf8_lossy(&result.stdout),
        &String::from_utf8_lossy(&result.stderr),
    ))
}

/// The agent name Claude Code's registration is attributed to (`X-Workgraph-Agent`).
pub const HTTP_AGENT: &str = "claude-code";

/// `_write(path, data)`: a timestamped copy of an existing file, then the new text written
/// atomically. Returns the backup's path when one was made.
fn write_settings(
    path: &Path,
    data: &serde_json::Map<String, Json>,
) -> Result<Option<String>, WgError> {
    let io_error = |action: &str, target: &Path, err: &std::io::Error| {
        WgError::workgraph(format!("could not {action} {}: {err}", target.display()))
    };
    let backup = if path.exists() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let now = time::OffsetDateTime::now_utc();
        let stamp = installer::backup_stamp(
            (now.year(), u8::from(now.month()), now.day()),
            (now.hour(), now.minute(), now.second()),
        );
        let target = path.with_file_name(installer::backup_name(&name, &stamp));
        std::fs::copy(path, &target).map_err(|err| io_error("copy", &target, &err))?;
        Some(target.display().to_string())
    } else {
        None
    };
    let text = installer::settings_text(data).map_err(installer::config_error)?;
    write_atomic(path, text.as_bytes()).map_err(|err| io_error("write", path, &err))?;
    Ok(backup)
}

/// The timeout of `claude mcp add` (`subprocess.run(..., timeout=60)`).
const MCP_ADD_TIMEOUT_MS: u64 = 60_000;

/// `--mcp`: `claude mcp add --scope {scope} workgraph -- {exe} mcp` when the `claude` CLI is on the
/// search path, else the manual hint with the MCP block.
fn register_mcp(scope: &str, exe: &str) -> Result<String, WgError> {
    let Some(claude) = find_on_path("claude") else {
        return installer::mcp_manual_hint(&installer::mcp_server_config(exe))
            .map_err(installer::config_error);
    };
    let argv = installer::mcp_add_argv(&claude.to_string_lossy(), scope, exe);
    let mut spec = ProcessSpec::new(argv);
    spec.timeout_ms = Some(MCP_ADD_TIMEOUT_MS);
    let result = run_capture(&spec, CaptureLimits::default(), None)
        .map_err(|err| WgError::workgraph(err.to_string()))?;
    if result.timed_out {
        return Err(WgError::workgraph(format!(
            "claude mcp add timed out after {} seconds",
            MCP_ADD_TIMEOUT_MS / 1000
        )));
    }
    Ok(installer::mcp_output(
        &String::from_utf8_lossy(&result.stdout),
        &String::from_utf8_lossy(&result.stderr),
    ))
}

/// `status`: the installation of `scope`, the ingestion statistics and the ten newest sessions.
pub fn status(
    app: &mut App,
    env: &ClaudeEnv,
    scope: &str,
    project_dir: Option<&Path>,
) -> Result<Json, WgError> {
    let installation = installation(env, scope, project_dir).to_object();
    let ingestion = hooks::hook_stats(app)?;
    let sessions = hooks::hook_sessions(app, None, 10)?;
    Ok(json!({
        "installation": installation,
        "ingestion": ingestion,
        "sessions": sessions,
    }))
}

/// `replay --limit`: ingests the spooled records, the first `limit` of them in name order. A
/// negative limit leaves out the last `|limit|` records, as Python's slice does.
pub fn replay(app: &mut App, limit: i64) -> Result<Json, WgError> {
    let count = if limit >= 0 {
        usize::try_from(limit).unwrap_or(usize::MAX)
    } else {
        spool_count(app).saturating_sub(limit.unsigned_abs().try_into().unwrap_or(usize::MAX))
    };
    let counts = hooks::hook_replay_spool(app, count)?;
    Ok(json!({
        "ingested": counts.ingested,
        "duplicate": counts.duplicate,
        "failed": counts.failed,
    }))
}

/// The number of `*.json` records in the spool directory.
fn spool_count(app: &App) -> usize {
    std::fs::read_dir(app.paths.spool_dir().as_str()).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count()
    })
}

/// `bind TASK --session --cwd --workspace`.
pub fn bind(
    app: &mut App,
    workspace: Option<&str>,
    task: &str,
    session: Option<&str>,
    cwd: Option<&Path>,
) -> Result<Json, WgError> {
    let context = human_context(None)?;
    let cwd_text = cwd.map(|path| path.to_string_lossy().into_owned());
    hooks::hook_bind(app, &context, workspace, task, session, cwd_text.as_deref())
}

/// `unbind --binding --cwd`: a binding id names one binding (an id that is not one matches none);
/// otherwise the working directory, resolved, names the bindings to deactivate.
pub fn unbind(app: &mut App, binding: Option<&str>, cwd: Option<&Path>) -> Result<usize, WgError> {
    match binding.filter(|text| !text.is_empty()) {
        Some(text) => match parse_id(text) {
            Ok(id) => hooks::hook_unbind(app, Some(id), None),
            Err(_) => Ok(0),
        },
        None => {
            let base = cwd
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            let resolved = hooks::resolved_path(&base.to_string_lossy());
            hooks::hook_unbind(app, None, Some(&resolved))
        }
    }
}

/// `sessions --limit`: the newest Claude Code sessions.
pub fn sessions(app: &mut App, limit: i64) -> Result<Vec<Json>, WgError> {
    hooks::hook_sessions(app, None, limit)
}

/// `backfill-transcripts --limit`.
pub fn backfill_transcripts(app: &mut App, limit: Option<i64>) -> Result<Json, WgError> {
    let context = human_context(None)?;
    hooks::hook_backfill_transcripts(app, &context, limit)
}

/// `backfill-time --limit`.
pub fn backfill_time(app: &mut App, limit: Option<i64>) -> Result<Json, WgError> {
    let context = human_context(None)?;
    hooks::hook_backfill_time(app, &context, limit)
}
