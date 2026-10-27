//! Shell: the agent adapters a run is launched through (`AgentRegistry` of
//! `integrations/agents/registry.py`): finding the configured adapter, probing whether it can be
//! used and running one attempt. The host is a trait so that tests launch a fake adapter instead
//! of a real agent process.

use serde_json::{Map, Value as Json};
use wg_agents::{
    AgentError, AgentEvent, AgentRunRequest, AgentRunResult, AgentRunner, ClaudeCodeProvider,
    ClaudeConfig, CommandAgentProvider, CommandConfig, describe_adapter,
};
use wg_model::py_repr_str;
use wg_proc::Cancel;
use wg_types::WgError;

/// What a run launches an agent through.
pub trait AgentHost {
    /// `check_available()` of the adapter the configuration entry describes.
    fn probe(&mut self, name: &str, entry: &Map<String, Json>) -> (bool, String);

    /// Runs one attempt; `cancel` is polled and ends the process once it answers true.
    fn launch(
        &mut self,
        name: &str,
        entry: &Map<String, Json>,
        request: &AgentRunRequest,
        on_event: Box<dyn FnMut(AgentEvent) + Send>,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError>;
}

/// The real adapters: the Claude Code CLI and the `workgraph.agent.v1` command agents.
pub struct LiveAgents;

fn is_claude(entry: &Map<String, Json>) -> bool {
    entry.get("type").and_then(Json::as_str) == Some("claude_code")
}

impl AgentHost for LiveAgents {
    fn probe(&mut self, name: &str, entry: &Map<String, Json>) -> (bool, String) {
        if is_claude(entry) {
            ClaudeCodeProvider::new(name, ClaudeConfig::from_config(entry)).check_available()
        } else {
            match CommandAgentProvider::new(name, CommandConfig::from_config(entry)) {
                Ok(provider) => provider.check_available(),
                Err(error) => (false, error.to_string()),
            }
        }
    }

    fn launch(
        &mut self,
        name: &str,
        entry: &Map<String, Json>,
        request: &AgentRunRequest,
        on_event: Box<dyn FnMut(AgentEvent) + Send>,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError> {
        if is_claude(entry) {
            ClaudeCodeProvider::new(name, ClaudeConfig::from_config(entry))
                .run(request, on_event, cancel)
        } else {
            CommandAgentProvider::new(name, CommandConfig::from_config(entry))?
                .run(request, on_event, cancel)
        }
    }
}

/// An adapter that is configured and usable.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub entry: Map<String, Json>,
    /// `provider.adapter_type`.
    pub adapter: &'static str,
    /// `provider.config_snapshot()`.
    pub snapshot: Map<String, Json>,
    /// What `check_available()` said about it (a path or a version).
    pub detail: String,
}

/// The `agents` table of a dumped configuration.
fn agents_of(config: &Map<String, Json>) -> Map<String, Json> {
    config
        .get("agents")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `AgentRegistry.get`: the configured adapter, or `not_found` naming the configured ones.
pub fn get(
    config: &Map<String, Json>,
    name: &str,
) -> Result<(Map<String, Json>, Provider), WgError> {
    let agents = agents_of(config);
    let entry = agents
        .get(name)
        .and_then(Json::as_object)
        .cloned()
        .ok_or_else(|| {
            let mut configured: Vec<&String> = agents.keys().collect();
            configured.sort();
            WgError::not_found(format!(
                "agent adapter {} is not configured",
                py_repr_str(name)
            ))
            .detail(
                "configured",
                Json::Array(
                    configured
                        .into_iter()
                        .map(|n| Json::String(n.clone()))
                        .collect(),
                ),
            )
        })?;
    let described = describe_adapter(&entry).map_err(|error| WgError::config(error.to_string()))?;
    let snapshot = described.config.as_object().cloned().unwrap_or_default();
    Ok((
        entry.clone(),
        Provider {
            name: name.to_owned(),
            entry,
            adapter: described.adapter,
            snapshot,
            detail: String::new(),
        },
    ))
}

/// `AgentRegistry.require_available`: the adapter, or `provider_unavailable` with the reason.
pub fn require_available(
    host: &mut dyn AgentHost,
    config: &Map<String, Json>,
    name: &str,
) -> Result<Provider, WgError> {
    let (entry, provider) = get(config, name)?;
    let (ok, detail) = host.probe(name, &entry);
    if ok {
        Ok(Provider { detail, ..provider })
    } else {
        Err(WgError::provider_unavailable(format!(
            "agent adapter {} is unavailable: {detail}",
            py_repr_str(name)
        )))
    }
}
