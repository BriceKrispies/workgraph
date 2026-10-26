//! IF-02 (provider part): the `llm provider <name>` and `agent adapter <name>` rows of `doctor`,
//! with the probes supplied by the test so that nothing reaches the network or a process.

#![allow(clippy::expect_used)]

use serde_json::{Map, Value, json};
use wg_registry::providers::ProviderHealth;
use wg_registry_feature::service::{HealthProbe, LiveProbe, provider_health};
use wg_registry_feature::spec::setup::{DoctorFacts, doctor_checks};

/// A probe that answers from fixed facts and records which models were checked.
struct Scripted {
    listing: Result<Vec<String>, String>,
    served: Vec<String>,
    agent: (bool, String),
    checked: Vec<String>,
}

impl HealthProbe for Scripted {
    fn list_models(
        &mut self,
        _name: &str,
        _entry: &Map<String, Value>,
    ) -> Result<Vec<String>, String> {
        self.listing.clone()
    }

    fn check_model(
        &mut self,
        _name: &str,
        _entry: &Map<String, Value>,
        model: &str,
    ) -> (bool, String) {
        self.checked.push(model.to_owned());
        if self.served.iter().any(|served| served == model) {
            (true, format!("{model} available"))
        } else {
            (false, format!("model {model:?} is not pulled"))
        }
    }

    fn agent(&mut self, _name: &str, _entry: &Map<String, Value>) -> (bool, String) {
        self.agent.clone()
    }
}

fn scripted(served: &[&str], agent: (bool, &str)) -> Scripted {
    Scripted {
        listing: Ok(vec!["qwen2.5:7b-instruct".to_owned()]),
        served: served.iter().map(|model| (*model).to_owned()).collect(),
        agent: (agent.0, agent.1.to_owned()),
        checked: Vec::new(),
    }
}

/// The dumped configuration of a default install with one agent adapter: the Ollama and heuristic
/// providers, the three model selections on Ollama.
fn default_config() -> Map<String, Value> {
    json!({
        "llm": {
            "providers": {
                "ollama": {"type": "ollama", "base_url": "http://127.0.0.1:11434"},
                "heuristic": {"type": "heuristic"}
            },
            "classification": {"provider": "ollama", "model": "qwen2.5:7b-instruct"},
            "embeddings": {"provider": "ollama", "model": "nomic-embed-text"},
            "summarization": {"provider": "ollama", "model": "qwen2.5:7b-instruct"}
        },
        "agents": {"claude_code": {"type": "claude_code", "executable": "claude"}}
    })
    .as_object()
    .cloned()
    .expect("object")
}

/// `default_config` without the Ollama provider, so that the live probe never reaches a server.
fn without_ollama(mut config: Map<String, Value>) -> Map<String, Value> {
    let providers = json!({"heuristic": {"type": "heuristic"}});
    if let Some(llm) = config.get_mut("llm").and_then(Value::as_object_mut) {
        llm.insert("providers".to_owned(), providers);
    }
    config
}

fn health(available: bool, detail: &str) -> ProviderHealth {
    ProviderHealth {
        available,
        detail: detail.to_owned(),
    }
}

// covers: C065
#[test]
fn llm_provider_checks_each_selected_model_once_in_selection_order() {
    let config = default_config();
    let mut probe = scripted(&["qwen2.5:7b-instruct"], (true, "2.1.0"));
    let (llm, _) = provider_health(&config, &mut probe);

    assert_eq!(
        probe.checked,
        vec!["qwen2.5:7b-instruct", "nomic-embed-text"],
        "the selections name the models; a repeated one is checked once"
    );
    let ollama = llm
        .iter()
        .find(|(name, _)| name == "ollama")
        .expect("ollama row");
    assert_eq!(
        ollama.1,
        health(
            false,
            "qwen2.5:7b-instruct available; model \"nomic-embed-text\" is not pulled"
        )
    );
}

// covers: C065
#[test]
fn llm_provider_without_selected_models_is_judged_by_its_listing() {
    let config = default_config();
    let mut probe = Scripted {
        listing: Ok(vec!["a".to_owned(), "b".to_owned()]),
        ..scripted(&[], (true, ""))
    };
    let (llm, _) = provider_health(&config, &mut probe);

    let heuristic = llm
        .iter()
        .find(|(name, _)| name == "heuristic")
        .expect("heuristic row");
    assert_eq!(heuristic.1, health(true, "2 models"));
    assert!(
        probe.checked.iter().all(|model| model != "heuristic"),
        "no selection names the heuristic provider"
    );
}

// covers: C065
#[test]
fn heuristic_provider_is_offline_and_serves_two_models() {
    let config = without_ollama(default_config());
    let (llm, _) = provider_health(&config, &mut LiveProbe);

    assert_eq!(llm.len(), 1);
    assert_eq!(llm[0].0, "heuristic");
    assert_eq!(llm[0].1, health(true, "2 models"));
}

// covers: C065
#[test]
fn missing_claude_code_executable_is_reported_by_the_live_adapter() {
    let mut config = without_ollama(default_config());
    config.insert(
        "agents".to_owned(),
        json!({"claude_code": {"type": "claude_code", "executable": "wg-no-such-executable-xyz"}}),
    );
    let (_, agents) = provider_health(&config, &mut LiveProbe);

    let (_, adapter) = &agents[0];
    assert!(!adapter.available);
    assert!(
        adapter.detail.contains("was not found on PATH"),
        "{}",
        adapter.detail
    );
    assert!(
        adapter
            .detail
            .ends_with("(install Claude Code or set agents.claude_code.executable)")
    );
}

// covers: C065
#[test]
fn doctor_rows_follow_python_order_and_levels() {
    let config = default_config();
    let mut probe = scripted(
        &["qwen2.5:7b-instruct", "nomic-embed-text"],
        (false, "not found"),
    );
    let (llm, agents) = provider_health(&config, &mut probe);
    let facts = DoctorFacts {
        current_revision: Some("0011".to_owned()),
        head_revision: Some("0011".to_owned()),
        integrity: "ok".to_owned(),
        journal_mode: "wal".to_owned(),
        languages: vec!["python".to_owned()],
        llm,
        agents,
        ..DoctorFacts::default()
    };
    let checks = doctor_checks(&facts);
    let names: Vec<&str> = checks.iter().map(|check| check.name.as_str()).collect();
    let position = |name: &str| {
        names
            .iter()
            .position(|candidate| *candidate == name)
            .expect(name)
    };

    let grammars = position("tree-sitter grammars");
    assert!(grammars < position("llm provider ollama"));
    assert!(position("llm provider ollama") < position("llm provider heuristic"));
    assert!(position("llm provider heuristic") < position("agent adapter claude_code"));
    assert!(position("agent adapter claude_code") < position("agent execution"));

    let adapter = checks
        .iter()
        .find(|check| check.name == "agent adapter claude_code")
        .expect("adapter row");
    assert!(!adapter.ok);
    assert_eq!(adapter.level, "warning");
    assert_eq!(adapter.detail, "not found");
    let heuristic = checks
        .iter()
        .find(|check| check.name == "llm provider heuristic")
        .expect("heuristic row");
    assert!(heuristic.ok);
    assert_eq!(
        heuristic.detail, "1 models",
        "the scripted listing has one model"
    );
}
