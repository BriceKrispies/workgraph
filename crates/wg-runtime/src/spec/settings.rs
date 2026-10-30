//! The `meta` document's inputs (`api/routes/ops.py`): what the configuration supplies to
//! `GET /api/v1/meta` (H002). The settings patch rules (`PATCH /api/v1/settings`) are the
//! registry feature's `spec::settings`.

use serde_json::{Map, Value};

use crate::spec::app::configured_default_workspace;

/// The fields of the `meta` document that the configuration supplies.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaInputs {
    pub default_workspace: Option<String>,
    pub execution_enabled: bool,
    pub auto_schedule: bool,
    /// The names of the configured agent adapters, in configuration order.
    pub agents: Vec<String>,
    /// `llm.classification` as dumped.
    pub classification: Value,
    /// `llm.embeddings` as dumped.
    pub embeddings: Value,
}

/// Reads the `meta` document's inputs from a dumped configuration.
pub fn meta_inputs(config: &Map<String, Value>) -> MetaInputs {
    let section = |name: &str| config.get(name).and_then(Value::as_object);
    let flag = |name: &str| {
        section("execution")
            .and_then(|execution| execution.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let llm_part = |name: &str| {
        section("llm")
            .and_then(|llm| llm.get(name))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()))
    };
    MetaInputs {
        default_workspace: configured_default_workspace(config),
        execution_enabled: flag("enabled"),
        auto_schedule: flag("auto_schedule"),
        agents: section("agents")
            .map(|agents| agents.keys().cloned().collect())
            .unwrap_or_default(),
        classification: llm_part("classification"),
        embeddings: llm_part("embeddings"),
    }
}

#[cfg(test)]
mod tests {
    use wg_config::default_global;

    use super::*;

    #[test]
    fn meta_reads_the_dumped_configuration() {
        let inputs = meta_inputs(&default_global());
        assert_eq!(inputs.default_workspace, None);
        assert!(!inputs.execution_enabled);
        assert!(!inputs.auto_schedule);
        assert!(!inputs.agents.is_empty());
        assert!(inputs.classification.is_object());
        assert!(inputs.embeddings.is_object());
    }
}
