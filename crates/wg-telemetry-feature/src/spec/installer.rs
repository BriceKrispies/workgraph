//! Pure: the Claude Code hook installation state and the MCP client block (`integrations/
//! claude_code/installer.py`, the parts `claude status` and `claude mcp-config` read).
//!
//! The settings path, the parse of the settings document, the recognition of Workgraph's own
//! handlers and the status are decisions of their inputs. The shell reads the file, the
//! environment and the disk, and passes them in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

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
    let command = object.get("command").and_then(Value::as_str).unwrap_or("");
    let forward = command.replace('\\', "/");
    let last = forward.rsplit('/').next().unwrap_or("");
    let stem = Path::new(last)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_lowercase();
    let exec_form = matches!(
        object.get("args"),
        Some(Value::Array(items))
            if items.first().and_then(Value::as_str) == Some(HOOK_ARGS[0])
                && items.get(1).and_then(Value::as_str) == Some(HOOK_ARGS[1])
    ) && !command.is_empty()
        && stem == "workgraph";
    exec_form || forward.contains("workgraph claude hook")
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
}
