//! The description of one agent adapter entry, as `AgentRegistry.describe` reports it besides the
//! probe's answer: the adapter type, the provider's capabilities and its non-secret configuration
//! snapshot.

use serde_json::{Map, Value};

use crate::claude::{ClaudeConfig, claude_capabilities, claude_config_snapshot};
use crate::command::{
    CommandConfig, command_capabilities, command_config_snapshot, validate_template,
};
use crate::error::AgentError;

/// What an adapter entry describes itself as.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterDescription {
    /// The adapter type (`adapter_type`): `claude_code` or `command`.
    pub adapter: &'static str,
    /// The capability map (`capabilities().to_dict()`), in declaration order.
    pub capabilities: Value,
    /// The non-secret configuration snapshot (`config_snapshot()`).
    pub config: Value,
}

/// The description of one adapter entry. A `claude_code` entry is a Claude Code adapter and any
/// other entry is a command adapter, as `build_provider` decides. A command entry whose argument
/// template uses an unknown placeholder fails, as `CommandAgentProvider.__init__` raises.
///
/// # Errors
/// [`AgentError::Config`] when a command entry's argument template is invalid.
pub fn describe_adapter(entry: &Map<String, Value>) -> Result<AdapterDescription, AgentError> {
    if entry.get("type").and_then(Value::as_str) == Some("claude_code") {
        let config = ClaudeConfig::from_config(entry);
        Ok(AdapterDescription {
            adapter: "claude_code",
            capabilities: claude_capabilities().to_json(),
            config: claude_config_snapshot(&config),
        })
    } else {
        let config = CommandConfig::from_config(entry);
        validate_template(&config.args).map_err(AgentError::Config)?;
        Ok(AdapterDescription {
            adapter: "command",
            capabilities: command_capabilities(&config).to_json(),
            config: command_config_snapshot(&config),
        })
    }
}
