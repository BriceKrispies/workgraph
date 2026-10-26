//! Shell: the Claude Code commands' services (`cli/commands/claude.py` and the installer calls of
//! `integrations/claude_code/installer.py`). The settings file, the environment, the executable
//! search and the hook ingestion are read here; the decisions are [`crate::spec::installer`].
//!
//! The services take the [`ClaudeEnv`] they read the home directory and `CLAUDE_CONFIG_DIR`
//! from, so tests run them against a temporary home and never touch the user's settings.

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use wg_runtime::{App, human_context};
use wg_types::{ErrorCode, WgError, parse_id};

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
            WgError::new(
                ErrorCode::ConfigError,
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
