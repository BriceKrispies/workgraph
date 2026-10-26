//! LLM provider and agent adapter health: `LLMRegistry.health` and `AgentRegistry.health`
//! (`integrations/llm/registry.py`, `integrations/agents/registry.py`), the decisions behind the
//! `llm provider <name>` and `agent adapter <name>` rows of `doctor`.
//!
//! The shell makes the probes (a provider's model listing and `check_model`, an adapter's
//! `check_available`). This module says which models a provider must serve, which provider a
//! configuration entry is, and what the probes' answers add up to. It reads the dumped
//! configuration and never touches the network or a process.

use serde_json::{Map, Value};

/// `{"available": ..., "detail": ...}`: the health of one provider or adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub available: bool,
    pub detail: String,
}

/// The providers of `llm.providers`, in configuration order.
#[must_use]
pub fn llm_providers(config: &Map<String, Value>) -> Vec<(&str, &Map<String, Value>)> {
    named_objects(config.get("llm").and_then(|llm| llm.get("providers")))
}

/// The entry of provider `name` in `llm.providers`, if it is configured (`LLMRegistry.get`).
#[must_use]
pub fn llm_provider<'a>(
    config: &'a Map<String, Value>,
    name: &str,
) -> Option<&'a Map<String, Value>> {
    llm_providers(config)
        .into_iter()
        .find(|(entry_name, _)| *entry_name == name)
        .map(|(_, entry)| entry)
}

/// The `(provider, model)` of the `capability` selection (`classification`, `embeddings` or
/// `summarization`) under `llm`, when both are set. `LLMRegistry.selection`.
#[must_use]
pub fn llm_selection<'a>(
    config: &'a Map<String, Value>,
    capability: &str,
) -> Option<(&'a str, &'a str)> {
    let selection = config.get("llm")?.get(capability)?;
    let provider = selection.get("provider")?.as_str()?;
    let model = selection.get("model")?.as_str()?;
    Some((provider, model))
}

/// The adapters of `agents`, in configuration order.
#[must_use]
pub fn agent_adapters(config: &Map<String, Value>) -> Vec<(&str, &Map<String, Value>)> {
    named_objects(config.get("agents"))
}

/// The objects of a name-keyed section, in their order. Other members are skipped.
fn named_objects(section: Option<&Value>) -> Vec<(&str, &Map<String, Value>)> {
    section
        .and_then(Value::as_object)
        .map_or_else(Vec::new, |entries| {
            entries
                .iter()
                .filter_map(|(name, entry)| entry.as_object().map(|entry| (name.as_str(), entry)))
                .collect()
        })
}

/// The `type` of a provider or adapter entry. A missing type is the heuristic provider, as
/// Python's `build` falls back to it.
#[must_use]
pub fn entry_type(entry: &Map<String, Value>) -> &str {
    entry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("heuristic")
}

/// The models `provider` must serve: the models of the `classification`, `embeddings` and
/// `summarization` selections that name it, in that order, each once (`dict.fromkeys(wanted)`).
/// Empty when no selection names the provider.
#[must_use]
pub fn wanted_models(config: &Map<String, Value>, provider: &str) -> Vec<String> {
    let llm = config.get("llm");
    ["classification", "embeddings", "summarization"]
        .into_iter()
        .filter_map(|capability| llm.and_then(|llm| llm.get(capability)))
        .filter(|selection| selection.get("provider").and_then(Value::as_str) == Some(provider))
        .filter_map(|selection| selection.get("model").and_then(Value::as_str))
        .fold(Vec::new(), |models: Vec<String>, model| {
            if models.iter().any(|seen| seen == model) {
                models
            } else {
                models
                    .into_iter()
                    .chain(std::iter::once(model.to_owned()))
                    .collect()
            }
        })
}

/// The health of one LLM provider (`LLMRegistry.health`) from the probes the shell made of it.
/// `listing` is the provider's model listing, made only when no model is wanted; `checks` are the
/// `check_model` answers for the wanted models, in order. Without wanted models the provider is
/// judged by its listing: available with its model count, or unavailable with the failure. With
/// wanted models it is available when every check is, and its detail joins their details.
#[must_use]
pub fn llm_health(
    listing: Option<Result<Vec<String>, String>>,
    checks: &[(bool, String)],
) -> ProviderHealth {
    match listing {
        Some(Ok(models)) => ProviderHealth {
            available: true,
            detail: format!("{} models", models.len()),
        },
        Some(Err(detail)) => ProviderHealth {
            available: false,
            detail,
        },
        None => ProviderHealth {
            available: checks.iter().all(|(ok, _)| *ok),
            detail: checks
                .iter()
                .map(|(_, detail)| detail.as_str())
                .collect::<Vec<&str>>()
                .join("; "),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config() -> Map<String, Value> {
        json!({
            "llm": {
                "providers": {"ollama": {"type": "ollama"}, "heuristic": {"type": "heuristic"}},
                "classification": {"provider": "ollama", "model": "qwen"},
                "embeddings": {"provider": "ollama", "model": "nomic"},
                "summarization": {"provider": "ollama", "model": "qwen"}
            }
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    }

    #[test]
    fn wanted_models_keep_selection_order_and_drop_repeats() {
        let config = config();
        assert_eq!(wanted_models(&config, "ollama"), vec!["qwen", "nomic"]);
        assert!(wanted_models(&config, "heuristic").is_empty());
    }

    #[test]
    fn llm_health_without_wanted_models_counts_the_listing() {
        let health = llm_health(Some(Ok(vec!["a".to_owned(), "b".to_owned()])), &[]);
        assert_eq!(
            health,
            ProviderHealth {
                available: true,
                detail: "2 models".to_owned()
            }
        );
        let down = llm_health(Some(Err("ollama at x is unreachable".to_owned())), &[]);
        assert_eq!(
            down,
            ProviderHealth {
                available: false,
                detail: "ollama at x is unreachable".to_owned()
            }
        );
    }

    #[test]
    fn llm_health_joins_every_check_and_needs_all_of_them() {
        let checks = vec![
            (true, "qwen checked".to_owned()),
            (false, "nomic checked".to_owned()),
        ];
        let health = llm_health(None, &checks);
        assert_eq!(
            health,
            ProviderHealth {
                available: false,
                detail: "qwen checked; nomic checked".to_owned()
            }
        );
    }
}
