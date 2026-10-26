//! The pure half of `init`, `doctor` and `config` (`cli/commands/core.py`): the clap commands,
//! the rules of `config set` and the human views of the results.

use clap::ArgMatches;
use serde_json::{Map, Value as Json};
use wg_config::{parse_value, set_path, validate_global, validate_overrides, value_repr};
use wg_json::Value;
use wg_render::Console;
use wg_wire::convention::json_value;

use crate::args::{get, items, opt, python, s};
use crate::tree::{SNAPSHOT_JSON, placeholder};
use crate::{CliFailure, Snapshot};

/// Exit code of a configuration error (`ConfigError`, `fail(..., 7)`).
const EXIT_CONFIG: u8 = 7;

/// The snapshot's own command for the top-level command or group `name`.
fn snapshot_command(name: &str) -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, name),
        Err(_) => crate::tree::command(name),
    }
}

/// The clap command of `init`.
#[must_use]
pub fn init_command() -> clap::Command {
    snapshot_command("init")
}

/// The clap command of `doctor`.
#[must_use]
pub fn doctor_command() -> clap::Command {
    snapshot_command("doctor")
}

/// The clap command of the `config` group.
#[must_use]
pub fn config_command() -> clap::Command {
    snapshot_command("config")
}

/// The options of `init`, with their defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitOptions {
    pub workspace: String,
    pub name: Option<String>,
    pub prefix: String,
    pub no_workspace: bool,
}

/// Reads the options of `init` (`--workspace default`, `--prefix WG`).
#[must_use]
pub fn init_options(m: &ArgMatches) -> InitOptions {
    let text = |id: &str| m.try_get_one::<String>(id).ok().flatten().cloned();
    InitOptions {
        workspace: text("--workspace").unwrap_or_else(|| "default".to_owned()),
        name: text("--name"),
        prefix: text("--prefix").unwrap_or_else(|| "WG".to_owned()),
        no_workspace: m
            .try_get_one::<bool>("--no-workspace")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false),
    }
}

/// Whether `--verify-events` was given to `doctor`.
#[must_use]
pub fn verify_events(m: &ArgMatches) -> bool {
    m.try_get_one::<bool>("--verify-events")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

/// What `config set` writes: the value as parsed and the validated document.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSet {
    pub parsed: Json,
    pub document: Map<String, Json>,
}

/// `config set KEY VALUE` up to the write: the value is parsed as JSON when it is JSON, stored
/// at the dotted key of `base` and the whole document validated (as the global configuration, or
/// as workspace overrides with `workspace`).
pub fn config_set(
    base: &Map<String, Json>,
    key: &str,
    raw: &str,
    workspace: bool,
) -> Result<ConfigSet, CliFailure> {
    let parsed = parse_value(raw);
    let target = set_path(base, key, parsed.clone())
        .ok_or_else(|| CliFailure::fail(format!("{key} does not address a nested setting"), 2))?;
    let validated = if workspace {
        validate_overrides(&Json::Object(target))
    } else {
        validate_global(&Json::Object(target))
    };
    match validated {
        Ok(document) => Ok(ConfigSet { parsed, document }),
        Err(errors) => Err(CliFailure::fail(
            format!(
                "invalid value for {key}: {}",
                errors
                    .first()
                    .map(|error| error.msg.as_str())
                    .unwrap_or_default()
            ),
            EXIT_CONFIG,
        )),
    }
}

/// The output of `config set`.
#[must_use]
pub fn config_set_value(key: &str, parsed: &Json, workspace: Option<&str>) -> Value {
    wg_wire::convention::dict(vec![
        ("key", wg_wire::convention::text(key)),
        ("value", json_value(parsed)),
        ("workspace", wg_wire::convention::optional_text(workspace)),
    ])
}

/// The human line of `config set`.
#[must_use]
pub fn config_set_line(console: &Console, key: &str, parsed: &Json) -> String {
    console
        .print(&format!("set {key} = {}", value_repr(parsed)))
        .text
}

/// `{"valid": ..., "problems": [...]}` of `config validate`.
#[must_use]
pub fn validate_value(problems: &[String]) -> Value {
    wg_wire::convention::dict(vec![
        ("valid", Value::Bool(problems.is_empty())),
        (
            "problems",
            Value::List(
                problems
                    .iter()
                    .map(|p| wg_wire::convention::text(p))
                    .collect(),
            ),
        ),
    ])
}

/// The result of `config show` as an output value.
#[must_use]
pub fn show_value(show: &Json) -> Value {
    json_value(show)
}

/// The human view of `init`.
#[must_use]
pub fn init_view(console: &Console, result: &Value) -> String {
    let keys = [
        "config_dir",
        "data_dir",
        "database",
        "schema_revision",
        "workspace_created",
        "default_workspace",
    ];
    let pairs: Vec<(&str, Option<String>)> = keys
        .iter()
        .map(|key| (*key, opt(get(result, key))))
        .collect();
    let borrowed: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, value.as_deref()))
        .collect();
    let mut out = console.print("[green]Workgraph initialised[/green]").text;
    out.push_str(&console.kv(&borrowed, None).text);
    out
}

/// The human view of `doctor`: one line per check, then the verdict.
#[must_use]
pub fn doctor_view(console: &Console, result: &Value) -> String {
    let mut out = String::new();
    for check in items(get(result, "checks")) {
        let ok = matches!(get(check, "ok"), Some(Value::Bool(true)));
        let mark = if ok {
            "[green]ok[/green]"
        } else if s(get(check, "level")) == "warning" {
            "[yellow]warn[/yellow]"
        } else {
            "[red]FAIL[/red]"
        };
        out.push_str(
            &console
                .print(&format!(
                    "{mark:>22}  {}: [dim]{}[/dim]",
                    s(get(check, "check")),
                    s(get(check, "detail"))
                ))
                .text,
        );
    }
    let healthy = matches!(get(result, "healthy"), Some(Value::Bool(true)));
    out.push_str(
        &console
            .print(if healthy {
                "[green]healthy[/green]"
            } else {
                "[red]problems found[/red]"
            })
            .text,
    );
    out
}

/// The human view of `config path`.
#[must_use]
pub fn paths_view(console: &Console, result: &Value) -> String {
    let Value::Dict(entries) = result else {
        return String::new();
    };
    let pairs: Vec<(String, String)> = entries
        .iter()
        .map(|(key, value)| (python(&Value::Str(key.clone())), python(value)))
        .collect();
    let borrowed: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), Some(value.as_str())))
        .collect();
    console.kv(&borrowed, None).text
}

/// The human view of `config validate`.
#[must_use]
pub fn validate_view(console: &Console, result: &Value) -> String {
    if matches!(get(result, "valid"), Some(Value::Bool(true))) {
        return console.print("[green]configuration is valid[/green]").text;
    }
    let problems: Vec<String> = items(get(result, "problems")).iter().map(python).collect();
    console.print(&problems.join("\n")).text
}

/// Whether `config validate` found problems (it then exits 7).
#[must_use]
pub fn has_problems(result: &Value) -> bool {
    !matches!(get(result, "valid"), Some(Value::Bool(true)))
}

/// The human view of `config migrate`.
#[must_use]
pub fn migrate_view(console: &Console, result: &Value) -> String {
    console
        .print(&format!(
            "migrated {} file(s)",
            items(get(result, "rewritten")).len()
        ))
        .text
}

/// The human view of `config show`: the result as indented JSON (`console.print_json`).
#[must_use]
pub fn show_view(show: &Json) -> String {
    let mut text = serde_json::to_string_pretty(show).unwrap_or_default();
    text.push('\n');
    text
}
