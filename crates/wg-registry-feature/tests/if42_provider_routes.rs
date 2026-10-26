//! IF-42 (provider part): the bodies of `GET /api/v1/agents` (H119) and `GET /api/v1/llm/health`
//! (H120), with the probes supplied by the test so that nothing reaches the network or a process.

#![allow(clippy::expect_used)]

use serde_json::{Map, Value, json};
use wg_registry_feature::service::{HealthProbe, agent_listing, llm_report};
use wg_types::ErrorCode;

/// A probe that answers from fixed facts and records what it was asked.
struct Fixed {
    listing: Result<Vec<String>, String>,
    served: Vec<String>,
    agent: (bool, String),
    listed: bool,
    checked: Vec<String>,
}

impl Fixed {
    fn new(agent: (bool, &str)) -> Self {
        Self {
            listing: Ok(Vec::new()),
            served: Vec::new(),
            agent: (agent.0, agent.1.to_owned()),
            listed: false,
            checked: Vec::new(),
        }
    }
}

impl HealthProbe for Fixed {
    fn list_models(
        &mut self,
        _name: &str,
        _entry: &Map<String, Value>,
    ) -> Result<Vec<String>, String> {
        self.listed = true;
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

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object")
}

/// A configuration with the Claude Code adapter and a command adapter, and the heuristic provider.
fn adapters() -> Map<String, Value> {
    object(json!({
        "llm": {
            "providers": {"heuristic": {"type": "heuristic"}}
        },
        "agents": {
            "claude_code": {"type": "claude_code", "executable": "claude", "model": "sonnet"},
            "tool": {"type": "command", "executable": "my-agent", "args": ["--json"]}
        }
    }))
}

/// The wire form of a body, as the server serialises it.
fn wire(value: &wg_json::Value) -> Value {
    let text = wg_json::http_json(value).expect("serialises");
    serde_json::from_str(&text).expect("json")
}

// covers: H119
#[test]
fn agents_list_each_adapter_in_configuration_order_with_its_probe_answer() {
    let mut probe = Fixed::new((true, "2.1.0 (Claude Code)"));
    let body = wire(&agent_listing(&adapters(), &mut probe).expect("listing"));

    let entries = body.as_array().expect("list");
    assert_eq!(entries.len(), 2);
    let first = &entries[0];
    assert_eq!(first["name"], "claude_code");
    assert_eq!(first["adapter"], "claude_code");
    assert_eq!(first["available"], true);
    assert_eq!(first["detail"], "2.1.0 (Claude Code)");
    assert_eq!(first["config"]["adapter"], "claude_code");
    assert_eq!(first["config"]["model"], "sonnet");
    assert_eq!(first["capabilities"]["session_resume"], true);

    let keys: Vec<&str> = first
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "name",
            "adapter",
            "available",
            "detail",
            "capabilities",
            "config"
        ]
    );

    let command = &entries[1];
    assert_eq!(command["name"], "tool");
    assert_eq!(command["adapter"], "command");
    assert_eq!(command["config"]["adapter"], "command");
    assert_eq!(command["capabilities"]["cancellation"], true);
    assert_eq!(command["capabilities"]["isolated_workspace"], true);
}

// covers: H119
#[test]
fn agents_report_an_unavailable_adapter_with_the_probe_detail() {
    let mut probe = Fixed::new((false, "claude: executable not found"));
    let body = wire(&agent_listing(&adapters(), &mut probe).expect("listing"));

    let entries = body.as_array().expect("list");
    assert!(entries.iter().all(|entry| entry["available"] == false));
    assert_eq!(entries[0]["detail"], "claude: executable not found");
}

// covers: H119
#[test]
fn agents_fail_when_a_command_template_names_an_unknown_placeholder() {
    let config = object(json!({
        "agents": {"tool": {"type": "command", "executable": "x", "args": ["{nope}"]}}
    }));
    let mut probe = Fixed::new((true, "ok"));
    match agent_listing(&config, &mut probe) {
        Ok(_) => panic!("an invalid template must fail the listing"),
        Err(error) => assert_eq!(error.code, ErrorCode::ConfigError),
    }
}

// covers: H120
#[test]
fn llm_health_judges_a_provider_by_its_listing_when_no_model_is_selected() {
    let config = adapters();
    let mut probe = Fixed {
        listing: Ok(vec!["a".to_owned(), "b".to_owned()]),
        ..Fixed::new((true, ""))
    };
    let body = wire(&llm_report(&config, &mut probe));

    assert_eq!(
        body,
        json!({"heuristic": {"available": true, "detail": "2 models"}})
    );
    assert!(probe.listed);
    assert!(probe.checked.is_empty());
}

// covers: H120
#[test]
fn llm_health_reports_a_failed_listing_with_its_reason() {
    let config = adapters();
    let mut probe = Fixed {
        listing: Err("heuristic is down".to_owned()),
        ..Fixed::new((true, ""))
    };
    let body = wire(&llm_report(&config, &mut probe));

    assert_eq!(
        body,
        json!({"heuristic": {"available": false, "detail": "heuristic is down"}})
    );
}

// covers: H120
#[test]
fn llm_health_checks_each_selected_model_and_needs_all_of_them() {
    let config = object(json!({
        "llm": {
            "providers": {"ollama": {"type": "ollama"}},
            "classification": {"provider": "ollama", "model": "qwen"},
            "embeddings": {"provider": "ollama", "model": "nomic"},
            "summarization": {"provider": "ollama", "model": "qwen"}
        }
    }));
    let mut probe = Fixed {
        served: vec!["qwen".to_owned()],
        ..Fixed::new((true, ""))
    };
    let body = wire(&llm_report(&config, &mut probe));

    assert_eq!(probe.checked, ["qwen", "nomic"]);
    assert!(!probe.listed);
    assert_eq!(
        body,
        json!({"ollama": {
            "available": false,
            "detail": "qwen available; model \"nomic\" is not pulled"
        }})
    );
}
