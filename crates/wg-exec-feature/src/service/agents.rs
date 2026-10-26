//! The `agent` command group's services: the listing of the configured adapters
//! (`AgentRegistry.describe`) and the registration of a command agent in the global configuration.

use serde_json::{Map, Value as Json, json};
use wg_agentspec::{PLACEHOLDERS, validate_template};
use wg_json::Value;
use wg_runtime::App;
use wg_types::{ErrorCode, WgError};
use wg_wire::convention::json_value;

/// `agents.describe()`: one entry per adapter in configuration order, with availability and
/// capabilities.
pub fn agent_list(app: &mut App) -> Result<Value, WgError> {
    wg_registry_feature::service::agents_document(app)
}

/// What `agent add-command` registers (`CommandAgentConfig`).
pub struct CommandAgent<'a> {
    pub name: &'a str,
    pub executable: &'a str,
    pub args: &'a [String],
    pub timeout_seconds: f64,
    pub supports_usage: bool,
}

/// `validate_template`'s refusal, as the `ConfigError` Python raises.
fn unknown_placeholder(name: &str) -> WgError {
    let mut allowed: Vec<&str> = PLACEHOLDERS.to_vec();
    allowed.sort_unstable();
    WgError::new(
        ErrorCode::ConfigError,
        format!("unknown placeholder {{{name}}} in agent argument template"),
    )
    .with_detail("allowed", json!(allowed))
}

/// `agent add-command`: registers a generic command agent under `name` in `config.toml`, replacing
/// an agent of that name. The reply names the agent.
pub fn agent_add_command(app: &mut App, agent: &CommandAgent<'_>) -> Result<Value, WgError> {
    validate_template(agent.args).map_err(|err| unknown_placeholder(&err.name))?;
    if agent.timeout_seconds.is_nan() || agent.timeout_seconds <= 0.0 {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "timeout_seconds must be greater than 0",
        ));
    }
    let mut config = app.config()?;
    let entry: Map<String, Json> = [
        ("type", json!("command")),
        ("executable", json!(agent.executable)),
        ("args", json!(agent.args)),
        ("env", json!({})),
        ("timeout_seconds", json!(agent.timeout_seconds)),
        ("supports_progress", json!(true)),
        ("supports_usage", json!(agent.supports_usage)),
        ("supports_resume", json!(false)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    if let Some(agents) = config
        .entry("agents")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        agents.insert(agent.name.to_owned(), Json::Object(entry));
    }
    app.save_config(&config)?;
    Ok(json_value(&json!({
        "name": agent.name,
        "executable": agent.executable,
        "args": agent.args,
    })))
}
