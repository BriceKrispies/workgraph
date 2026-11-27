//! Pure: the Claude Code hook installation state and the MCP client block (`integrations/
//! claude_code/installer.py`, the parts `claude status` and `claude mcp-config` read).
//!
//! The settings path, the parse of the settings document, the recognition of Workgraph's own
//! handlers and the status are decisions of their inputs. The shell reads the file, the
//! environment and the disk, and passes them in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use wg_types::WgError;
use wg_types::pyval::{prefix, text};

/// `DEFAULT_EVENTS`: the hook events Workgraph installs, in Python's order.
pub const DEFAULT_EVENTS: [&str; 10] = [
    "SessionStart",
    "SessionEnd",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
];

/// `HOOK_ARGS`: the exec-form arguments of a Workgraph hook handler.
const HOOK_ARGS: [&str; 2] = ["claude", "hook"];

/// What `installer.status` reports for one scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallerStatus {
    pub scope: String,
    /// `str(path)` of the settings file.
    pub settings: String,
    pub exists: bool,
    /// The `ConfigError` text when the settings file cannot be used.
    pub error: Option<String>,
    pub installed_events: Vec<String>,
    pub missing_events: Vec<String>,
    pub command_resolves: bool,
}

impl InstallerStatus {
    /// The status as the Python dict has it, keys in Python's order.
    #[must_use]
    pub fn to_object(&self) -> Map<String, Value> {
        let names = |items: &[String]| Value::Array(items.iter().map(|s| json!(s)).collect());
        match json!({
            "scope": self.scope,
            "settings": self.settings,
            "exists": self.exists,
            "error": self.error,
            "installed_events": names(&self.installed_events),
            "missing_events": names(&self.missing_events),
            "command_resolves": self.command_resolves,
        }) {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }
}

/// `settings_path(scope, project_dir)`. `config_dir` is `CLAUDE_CONFIG_DIR` when it is set, `home`
/// is `Path.home()` and `root` is the resolved project directory (used by the project and local
/// scopes only). Any scope other than `user`, `project` names the local file, as Python does.
#[must_use]
pub fn settings_path(scope: &str, config_dir: Option<&Path>, home: &Path, root: &Path) -> PathBuf {
    if scope == "user" {
        let base = config_dir.map_or_else(|| home.join(".claude"), Path::to_path_buf);
        return base.join("settings.json");
    }
    let file = if scope == "project" {
        "settings.json"
    } else {
        "settings.local.json"
    };
    root.join(".claude").join(file)
}

/// `_load(path)` on the text of the file: an empty document is `{}`; an unparsable one or one
/// that is not a JSON object is refused with the `ConfigError` text of the Python code.
pub fn parse_settings(path: &str, text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match wg_validate::pyjson::parse_message(text) {
        Err(exc) => Err(format!(
            "{path} is not valid JSON ({exc}); refusing to modify it"
        )),
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!(
            "{path} does not contain a JSON object; refusing to modify it"
        )),
    }
}

/// `is_workgraph_handler`: a command handler that runs `workgraph claude hook` in exec form, or
/// the shell-form string `workgraph claude hook`.
#[must_use]
pub fn is_workgraph_handler(handler: &Value) -> bool {
    let Some(object) = handler.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("command") {
        return false;
    }
    let command = text(object.get("command"));
    let forward = command.replace('\\', "/");
    let exec_form = matches!(
        object.get("args"),
        Some(Value::Array(items))
            if items.first().and_then(Value::as_str) == Some(HOOK_ARGS[0])
                && items.get(1).and_then(Value::as_str) == Some(HOOK_ARGS[1])
    ) && is_workgraph_executable(command);
    exec_form || forward.contains("workgraph claude hook")
}

/// Whether `command` names the workgraph executable, by its file stem, in any directory.
fn is_workgraph_executable(command: &str) -> bool {
    let forward = command.replace('\\', "/");
    let last = forward.rsplit('/').next().unwrap_or("");
    let stem = Path::new(last)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_lowercase();
    !command.is_empty() && stem == "workgraph"
}

/// Whether `handler` is the SessionStart handler that starts the daemon: the exec form of the
/// workgraph executable with `serve --ensure --passive` (WG-5702).
#[must_use]
pub fn is_workgraph_ensure_handler(handler: &Value) -> bool {
    let Some(object) = handler.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("command") {
        return false;
    }
    let args_match = matches!(
        object.get("args"),
        Some(Value::Array(items))
            if items.len() == ENSURE_ARGS.len()
                && items
                    .iter()
                    .zip(ENSURE_ARGS)
                    .all(|(item, want)| item.as_str() == Some(want))
    );
    args_match && is_workgraph_executable(text(object.get("command")))
}

/// Whether Workgraph owns `handler`: a hook handler or the daemon's ensure handler. Uninstall
/// removes both.
#[must_use]
pub fn is_owned_handler(handler: &Value) -> bool {
    is_workgraph_handler(handler) || is_workgraph_ensure_handler(handler)
}

/// The command of each event that has a Workgraph handler (the last one wins, as the Python dict
/// assignment does).
#[must_use]
pub fn installed_events(settings: &Map<String, Value>) -> BTreeMap<String, String> {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    hooks
        .iter()
        .filter_map(|(event, groups)| groups.as_array().map(|groups| (event, groups)))
        .flat_map(|(event, groups)| {
            groups
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|group| group.get("hooks").and_then(Value::as_array))
                .flatten()
                .filter(|handler| is_workgraph_handler(handler))
                .map(move |handler| {
                    let command = handler.get("command").and_then(Value::as_str);
                    (event.clone(), command.unwrap_or("").to_owned())
                })
        })
        .collect()
}

/// `installer.status(scope, project_dir)` over what the shell read: `loaded` is the parsed
/// settings or the refusal, `resolves` tells whether a handler's command exists.
pub fn status(
    scope: &str,
    settings: &str,
    exists: bool,
    loaded: Result<Map<String, Value>, String>,
    resolves: impl Fn(&str) -> bool,
) -> InstallerStatus {
    let (data, error) = match loaded {
        Ok(data) => (data, None),
        Err(message) => (Map::new(), Some(message)),
    };
    let installed = installed_events(&data);
    let commands_ok = installed.values().all(|command| resolves(command));
    let missing: BTreeSet<String> = DEFAULT_EVENTS
        .iter()
        .filter(|event| !installed.contains_key(**event))
        .map(|event| (*event).to_owned())
        .collect();
    InstallerStatus {
        scope: scope.to_owned(),
        settings: settings.to_owned(),
        exists,
        error,
        installed_events: installed.keys().cloned().collect(),
        missing_events: missing.into_iter().collect(),
        command_resolves: !installed.is_empty() && commands_ok,
    }
}

/// `mcp_server_config(command)`: the block an MCP client needs to launch Workgraph over stdio.
#[must_use]
pub fn mcp_server_config(command: &str) -> Value {
    json!({"mcpServers": {"workgraph": {"type": "stdio", "command": command, "args": ["mcp"]}}})
}

/// The `timeout` of an installed hook handler (`install(timeout=10)`).
pub const HOOK_TIMEOUT: i64 = 10;

/// The exec-form arguments of the SessionStart handler that ensures the daemon (WG-5702).
pub const ENSURE_ARGS: [&str; 3] = ["serve", "--ensure", "--passive"];

/// The timeout of the ensure handler: starting the daemon waits for it to answer.
pub const ENSURE_TIMEOUT: i64 = 30;

/// The matcher group of the daemon's ensure handler: an async, passive exec-form handler.
#[must_use]
pub fn ensure_group(command: &str) -> Value {
    json!({
        "matcher": "",
        "hooks": [{
            "type": "command",
            "command": command,
            "args": ENSURE_ARGS,
            "timeout": ENSURE_TIMEOUT,
            "async": true
        }]
    })
}

/// The `install` edit for the daemon: adds the ensure group to SessionStart unless one of its
/// groups already has the ensure handler. Returns the edited document and whether it was added.
pub fn install_ensure(
    mut data: Map<String, Value>,
    path: &str,
    command: &str,
) -> Result<(Map<String, Value>, bool), String> {
    let Value::Object(hooks) = data
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return Err(format!(
            "'hooks' in {path} is not an object; refusing to modify it"
        ));
    };
    let Value::Array(groups) = hooks
        .entry("SessionStart")
        .or_insert_with(|| Value::Array(Vec::new()))
    else {
        return Err(format!(
            "hooks.SessionStart in {path} is not a list; refusing to modify it"
        ));
    };
    let present = groups
        .iter()
        .filter_map(Value::as_object)
        .any(|group| handlers_of(group).iter().any(is_workgraph_ensure_handler));
    if !present {
        groups.push(ensure_group(command));
    }
    Ok((data, !present))
}

/// The per-user logon autostart file: a command file for the Startup folder that starts the daemon
/// without a window (WG-5702).
#[must_use]
pub fn autostart_text(command: &str) -> String {
    format!("@echo off\r\nstart \"\" /B \"{command}\" serve --ensure --passive\r\n")
}

/// The argument vector of `claude mcp remove workgraph -s {scope}`.
#[must_use]
pub fn mcp_remove_argv(claude: &str, scope: &str) -> Vec<String> {
    [claude, "mcp", "remove", "workgraph", "-s", scope]
        .iter()
        .map(|part| (*part).to_owned())
        .collect()
}

/// The request header that names the agent over HTTP (`wg_iface::mcp::options::AGENT_HEADER`).
pub const HTTP_AGENT_HEADER: &str = "X-Workgraph-Agent";

/// The argument vector of the HTTP registration: `claude mcp add --transport http -s {scope}
/// workgraph {url}` with the bearer token and the agent-name header (WG-5702).
#[must_use]
pub fn mcp_add_http_argv(
    claude: &str,
    scope: &str,
    url: &str,
    token: &str,
    agent: &str,
) -> Vec<String> {
    vec![
        claude.to_owned(),
        "mcp".to_owned(),
        "add".to_owned(),
        "--transport".to_owned(),
        "http".to_owned(),
        "-s".to_owned(),
        scope.to_owned(),
        "workgraph".to_owned(),
        url.to_owned(),
        "--header".to_owned(),
        format!("Authorization: Bearer {token}"),
        "--header".to_owned(),
        format!("{HTTP_AGENT_HEADER}: {agent}"),
    ]
}

/// The matcher group Workgraph adds for one event: an async, passive exec-form handler.
#[must_use]
pub fn hook_group(command: &str) -> Value {
    json!({
        "matcher": "",
        "hooks": [{
            "type": "command",
            "command": command,
            "args": HOOK_ARGS,
            "timeout": HOOK_TIMEOUT,
            "async": true
        }]
    })
}

/// The items Python iterates for `group.get("hooks") or []`: a list yields its items, a
/// non-empty string its characters and a non-empty object its keys. Any other truthy value is a
/// Python `TypeError` the Rust port does not reproduce; it is read as no handlers.
fn handlers_of(group: &Map<String, Value>) -> Vec<Value> {
    match group.get("hooks") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => text.chars().map(|c| Value::String(c.to_string())).collect(),
        Some(Value::Object(members)) => members
            .keys()
            .map(|key| Value::String(key.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// `any(is_workgraph_handler(h) for g in groups if isinstance(g, dict) for h in g.hooks or [])`.
fn has_workgraph_handler(groups: &[Value]) -> bool {
    groups
        .iter()
        .filter_map(Value::as_object)
        .any(|group| handlers_of(group).iter().any(is_workgraph_handler))
}

/// The `install` edit of a loaded settings document: `hooks` is created when absent, and each
/// event in `events` gets a Workgraph group unless one of its groups already has a handler.
/// Returns the edited document and the events that were added, in order. `path` names the file in
/// the refusals.
pub fn install_hooks(
    mut data: Map<String, Value>,
    path: &str,
    events: &[String],
    command: &str,
) -> Result<(Map<String, Value>, Vec<String>), String> {
    let Value::Object(hooks) = data
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return Err(format!(
            "'hooks' in {path} is not an object; refusing to modify it"
        ));
    };
    let mut added = Vec::new();
    for event in events {
        let Value::Array(groups) = hooks
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()))
        else {
            return Err(format!(
                "hooks.{event} in {path} is not a list; refusing to modify it"
            ));
        };
        if has_workgraph_handler(groups) {
            continue;
        }
        groups.push(hook_group(command));
        added.push(event.clone());
    }
    Ok((data, added))
}

/// The `uninstall` edit: removes the Workgraph handlers from every event, drops the groups that
/// end up with no handler (and events left with no group), and drops `hooks` when it is empty.
/// Returns the edited document and the event of every group that lost a handler, repeats included.
pub fn uninstall_hooks(mut data: Map<String, Value>) -> (Map<String, Value>, Vec<String>) {
    let mut removed = Vec::new();
    let mut drop_hooks = false;
    if let Some(Value::Object(hooks)) = data.get_mut("hooks") {
        let mut kept_events = Map::new();
        for (event, groups) in std::mem::take(hooks) {
            let Value::Array(groups) = groups else {
                kept_events.insert(event, groups);
                continue;
            };
            let new_groups = uninstall_groups(&event, groups, &mut removed);
            if !new_groups.is_empty() {
                kept_events.insert(event, Value::Array(new_groups));
            }
        }
        drop_hooks = kept_events.is_empty();
        *hooks = kept_events;
    }
    if drop_hooks {
        data.shift_remove("hooks");
    }
    (data, removed)
}

/// The `_load` refusals, which are `ConfigError`s (exit 7).
#[must_use]
pub fn config_error(message: String) -> WgError {
    WgError::config(message)
}

/// The groups of one event after the uninstall edit; `removed` collects the event per changed group.
fn uninstall_groups(event: &str, groups: Vec<Value>, removed: &mut Vec<String>) -> Vec<Value> {
    let mut new_groups = Vec::new();
    for group in groups {
        let Value::Object(members) = &group else {
            new_groups.push(group);
            continue;
        };
        let handlers = handlers_of(members);
        let kept: Vec<Value> = handlers
            .iter()
            .filter(|handler| !is_owned_handler(handler))
            .cloned()
            .collect();
        if kept.len() != handlers.len() {
            removed.push(event.to_owned());
        }
        if !kept.is_empty() {
            let mut rebuilt = members.clone();
            rebuilt.insert("hooks".to_owned(), Value::Array(kept));
            new_groups.push(Value::Object(rebuilt));
        } else if handlers.is_empty() {
            new_groups.push(group);
        }
    }
    new_groups
}

/// The settings text Python writes: `json.dumps(data, indent=2) + "\n"`.
pub fn settings_text(data: &Map<String, Value>) -> Result<String, String> {
    let document = wg_wire::convention::json_object(data);
    wg_json::cli_json(&document).map_err(|err| err.to_string())
}

/// The `backup` file name: `{name}.workgraph-backup-{stamp}`, the stamp being `%Y%m%dT%H%M%SZ`.
#[must_use]
pub fn backup_name(file_name: &str, stamp: &str) -> String {
    format!("{file_name}.workgraph-backup-{stamp}")
}

/// `datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")` from its fields.
#[must_use]
pub fn backup_stamp(
    (year, month, day): (i32, u8, u8),
    (hour, minute, second): (u8, u8, u8),
) -> String {
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// The result of `installer.install`, keys in Python's order.
#[must_use]
pub fn install_object(
    settings: &str,
    added: &[String],
    backup: Option<&str>,
    command: &str,
) -> Map<String, Value> {
    match json!({
        "settings": settings,
        "added": added,
        "backup": backup,
        "command": command,
    }) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The result of `installer.uninstall`: the removed events sorted and without repeats.
#[must_use]
pub fn uninstall_object(
    settings: &str,
    removed: &[String],
    backup: Option<&str>,
) -> Map<String, Value> {
    let unique: BTreeSet<&String> = removed.iter().collect();
    let sorted: Vec<&String> = unique.into_iter().collect();
    match json!({
        "settings": settings,
        "removed": sorted,
        "backup": backup,
    }) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The `mcp` value when the `claude` CLI is not on the search path: a hint with the block as
/// Python's `json.dumps` writes it (default separators, ASCII).
pub fn mcp_manual_hint(config: &Value) -> Result<String, String> {
    let line = wg_json::log_json(&wg_wire::convention::json_value(config))
        .map_err(|err| err.to_string())?;
    Ok(format!("claude CLI not found; add manually: {line}"))
}

/// The argument vector of `claude mcp add --scope {scope} workgraph -- {command} mcp`.
#[must_use]
pub fn mcp_add_argv(claude: &str, scope: &str, command: &str) -> Vec<String> {
    [
        claude,
        "mcp",
        "add",
        "--scope",
        scope,
        "workgraph",
        "--",
        command,
        "mcp",
    ]
    .iter()
    .map(|part| (*part).to_owned())
    .collect()
}

/// The `mcp` value of a finished `claude mcp add`: `(stdout or stderr).strip()[:500]`.
#[must_use]
pub fn mcp_output(stdout: &str, stderr: &str) -> String {
    let text = if stdout.is_empty() { stderr } else { stdout };
    prefix(text.trim(), 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_picks_the_settings_file() {
        let home = Path::new("/home/u");
        let root = Path::new("/work/p");
        assert_eq!(
            settings_path("user", None, home, root),
            home.join(".claude").join("settings.json")
        );
        assert_eq!(
            settings_path("user", Some(Path::new("/cfg")), home, root),
            Path::new("/cfg").join("settings.json")
        );
        assert_eq!(
            settings_path("project", None, home, root),
            root.join(".claude").join("settings.json")
        );
        assert_eq!(
            settings_path("local", None, home, root),
            root.join(".claude").join("settings.local.json")
        );
    }

    #[test]
    fn a_refused_document_carries_the_python_message() {
        let text = "{\"a\" 1}";
        let error = parse_settings("/s.json", text).err().unwrap_or_default();
        assert_eq!(
            error,
            "/s.json is not valid JSON (Expecting ':' delimiter: line 1 column 6 (char 5)); refusing to modify it"
        );
        assert_eq!(
            parse_settings("/s.json", "[]").err().unwrap_or_default(),
            "/s.json does not contain a JSON object; refusing to modify it"
        );
        assert_eq!(parse_settings("/s.json", "  \n"), Ok(Map::new()));
    }

    #[test]
    fn workgraph_handlers_are_recognised_in_both_forms() {
        let exec = json!({"type": "command", "command": "C:\\bin\\WorkGraph.exe", "args": ["claude", "hook"]});
        let shell = json!({"type": "command", "command": "workgraph claude hook"});
        let other = json!({"type": "command", "command": "/bin/other", "args": ["claude", "hook"]});
        assert!(is_workgraph_handler(&exec));
        assert!(is_workgraph_handler(&shell));
        assert!(!is_workgraph_handler(&other));
        assert!(!is_workgraph_handler(
            &json!({"type": "http", "command": "workgraph claude hook"})
        ));
    }

    #[test]
    fn status_lists_installed_and_missing_events_sorted() {
        let settings = json!({"hooks": {
            "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "/x/workgraph", "args": ["claude", "hook"]}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "/usr/bin/other"}]}],
        }});
        let Value::Object(map) = settings else {
            return;
        };
        let status = status("user", "/s.json", true, Ok(map), |_| true);
        assert_eq!(status.installed_events, ["Stop"]);
        assert_eq!(
            status.missing_events.first().map(String::as_str),
            Some("PostToolUse")
        );
        assert_eq!(status.missing_events.len(), 9);
        assert!(status.command_resolves);
        assert_eq!(status.error, None);
    }

    #[test]
    fn mcp_config_is_the_stdio_block() {
        assert_eq!(
            mcp_server_config("/x/workgraph").to_string(),
            r#"{"mcpServers":{"workgraph":{"type":"stdio","command":"/x/workgraph","args":["mcp"]}}}"#
        );
    }

    #[test]
    fn the_daemon_handler_is_recognised_and_owned() {
        let exe = "C:/Users/u/.local/bin/workgraph.exe";
        let ensure = ensure_group(exe);
        let handler = &ensure["hooks"][0];
        assert!(is_workgraph_ensure_handler(handler));
        assert!(is_owned_handler(handler));
        assert!(!is_workgraph_handler(handler));
        let hook = hook_group(exe);
        assert!(!is_workgraph_ensure_handler(&hook["hooks"][0]));
    }

    #[test]
    fn install_ensure_adds_one_session_start_handler() {
        let exe = "C:/x/workgraph.exe";
        let (data, added) = install_ensure(Map::new(), "s.json", exe).expect("installs");
        assert!(added);
        let (data, again) = install_ensure(data, "s.json", exe).expect("installs");
        assert!(!again);
        let groups = data["hooks"]["SessionStart"].as_array().map_or(0, Vec::len);
        assert_eq!(groups, 1);
    }

    #[test]
    fn uninstall_removes_the_daemon_handler_with_the_hooks() {
        let exe = "C:/x/workgraph.exe";
        let (data, _) = install_hooks(Map::new(), "s.json", &["SessionStart".to_owned()], exe)
            .expect("installs");
        let (data, _) = install_ensure(data, "s.json", exe).expect("installs");
        let (data, removed) = uninstall_hooks(data);
        assert_eq!(removed.len(), 2);
        assert!(!data.contains_key("hooks"));
    }

    #[test]
    fn the_http_registration_names_the_endpoint_token_and_agent() {
        let argv = mcp_add_http_argv(
            "claude",
            "user",
            "http://127.0.0.1:8765/mcp/",
            "t0k",
            "claude-code",
        );
        assert_eq!(
            argv[5..],
            [
                "-s",
                "user",
                "workgraph",
                "http://127.0.0.1:8765/mcp/",
                "--header",
                "Authorization: Bearer t0k",
                "--header",
                "X-Workgraph-Agent: claude-code",
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            mcp_remove_argv("claude", "user"),
            ["claude", "mcp", "remove", "workgraph", "-s", "user"].map(str::to_owned)
        );
    }
}
