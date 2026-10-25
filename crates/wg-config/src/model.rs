//! The configuration models of `src/workgraph/config/settings.py`, in declaration order, and the
//! entry points that validate a document against them.

use serde_json::{Map, Value, json};
use wg_validate::ValidationError;

use crate::schema::{Field, Model, Schema};
use crate::validate::validate_model;

/// Declared bounds of an integer field.
fn int(ge: Option<i64>, le: Option<i64>) -> Schema {
    Schema::Int { ge, le }
}

/// A field with a default input.
fn def(name: &'static str, schema: Schema, default: Value) -> Field {
    Field {
        name,
        schema,
        default: Some(default),
    }
}

/// A required field.
fn req(name: &'static str, schema: Schema) -> Field {
    Field {
        name,
        schema,
        default: None,
    }
}

fn nullable(schema: Schema) -> Schema {
    Schema::Nullable(Box::new(schema))
}

fn list_of(item: Schema) -> Schema {
    Schema::List {
        item: Box::new(item),
        min_len: 0,
    }
}

fn dict_of(value: Schema) -> Schema {
    Schema::Dict(Box::new(value))
}

fn model(model: Model) -> Schema {
    Schema::Model(model)
}

fn ollama_provider() -> Model {
    Model {
        name: "OllamaProviderConfig",
        fields: vec![
            def("type", Schema::Literal(&["ollama"]), json!("ollama")),
            def("base_url", Schema::Str, json!("http://127.0.0.1:11434")),
            def(
                "timeout_seconds",
                Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                },
                json!(120.0),
            ),
            def("max_retries", int(Some(0), Some(10)), json!(2)),
            def(
                "backoff_seconds",
                Schema::Float {
                    ge: Some(0.0),
                    gt: None,
                },
                json!(1.0),
            ),
            def("api_key_env", nullable(Schema::Str), Value::Null),
        ],
    }
}

fn heuristic_provider() -> Model {
    Model {
        name: "HeuristicProviderConfig",
        fields: vec![def(
            "type",
            Schema::Literal(&["heuristic"]),
            json!("heuristic"),
        )],
    }
}

fn openai_compatible_provider() -> Model {
    Model {
        name: "OpenAICompatibleProviderConfig",
        fields: vec![
            def(
                "type",
                Schema::Literal(&["openai_compatible"]),
                json!("openai_compatible"),
            ),
            req("base_url", Schema::Str),
            def(
                "timeout_seconds",
                Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                },
                json!(120.0),
            ),
            def("max_retries", int(Some(0), Some(10)), json!(2)),
            def(
                "backoff_seconds",
                Schema::Float {
                    ge: Some(0.0),
                    gt: None,
                },
                json!(1.0),
            ),
            def("api_key_env", nullable(Schema::Str), Value::Null),
        ],
    }
}

fn model_selection() -> Model {
    Model {
        name: "ModelSelection",
        fields: vec![req("provider", Schema::Str), req("model", Schema::Str)],
    }
}

fn server() -> Model {
    Model {
        name: "ServerConfig",
        fields: vec![
            def(
                "host",
                Schema::NonBlank {
                    message: "host must not be empty",
                },
                json!("127.0.0.1"),
            ),
            def("port", int(Some(1), Some(65535)), json!(8765)),
            def("allow_non_loopback", Schema::Bool, json!(false)),
            def("lan", Schema::Bool, json!(true)),
            def("lan_address", nullable(Schema::Str), Value::Null),
            def("lan_networks", Schema::Networks, json!([])),
        ],
    }
}

fn llm() -> Model {
    Model {
        name: "LLMConfig",
        fields: vec![
            def(
                "providers",
                dict_of(Schema::Tagged(vec![
                    ("ollama", ollama_provider()),
                    ("heuristic", heuristic_provider()),
                    ("openai_compatible", openai_compatible_provider()),
                ])),
                json!({"ollama": {"type": "ollama"}, "heuristic": {"type": "heuristic"}}),
            ),
            def(
                "classification",
                model(model_selection()),
                json!({"provider": "ollama", "model": "qwen2.5:7b-instruct"}),
            ),
            def(
                "embeddings",
                model(model_selection()),
                json!({"provider": "ollama", "model": "nomic-embed-text"}),
            ),
            def(
                "summarization",
                model(model_selection()),
                json!({"provider": "ollama", "model": "qwen2.5:7b-instruct"}),
            ),
            def("auto_classify", Schema::Bool, json!(true)),
            def("auto_embed", Schema::Bool, json!(true)),
        ],
    }
}

fn claude_code_agent() -> Model {
    Model {
        name: "ClaudeCodeAgentConfig",
        fields: vec![
            def(
                "type",
                Schema::Literal(&["claude_code"]),
                json!("claude_code"),
            ),
            def("executable", Schema::Str, json!("claude")),
            def("model", nullable(Schema::Str), Value::Null),
            def(
                "permission_mode",
                Schema::Literal(&[
                    "acceptEdits",
                    "auto",
                    "bypassPermissions",
                    "manual",
                    "dontAsk",
                    "plan",
                ]),
                json!("acceptEdits"),
            ),
            def("allowed_tools", list_of(Schema::Str), json!([])),
            def("max_turns", nullable(int(Some(1), None)), Value::Null),
            def(
                "max_budget_usd",
                nullable(Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                }),
                Value::Null,
            ),
            def(
                "timeout_seconds",
                Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                },
                json!(3600.0),
            ),
            def("extra_args", list_of(Schema::Str), json!([])),
        ],
    }
}

fn command_agent() -> Model {
    Model {
        name: "CommandAgentConfig",
        fields: vec![
            def("type", Schema::Literal(&["command"]), json!("command")),
            req("executable", Schema::Str),
            def("args", list_of(Schema::Str), json!([])),
            def("env", dict_of(Schema::Str), json!({})),
            def(
                "timeout_seconds",
                Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                },
                json!(1800.0),
            ),
            def("supports_progress", Schema::Bool, json!(true)),
            def("supports_usage", Schema::Bool, json!(false)),
            def("supports_resume", Schema::Bool, json!(false)),
        ],
    }
}

fn agents_union() -> Schema {
    Schema::Tagged(vec![
        ("claude_code", claude_code_agent()),
        ("command", command_agent()),
    ])
}

fn execution() -> Model {
    Model {
        name: "ExecutionConfig",
        fields: vec![
            def("enabled", Schema::Bool, json!(false)),
            def("default_agent", Schema::Str, json!("claude_code")),
            def("max_concurrent_runs", int(Some(1), Some(64)), json!(2)),
            def("max_attempts_per_task", int(Some(1), Some(20)), json!(3)),
            def("auto_schedule", Schema::Bool, json!(false)),
            def(
                "isolation",
                Schema::Literal(&["worktree", "in_place"]),
                json!("worktree"),
            ),
            def("allow_push", Schema::Bool, json!(false)),
            def("allow_merge", Schema::Bool, json!(false)),
        ],
    }
}

fn jobs() -> Model {
    Model {
        name: "JobLimitsConfig",
        fields: vec![
            def("worker_concurrency", int(Some(1), Some(64)), json!(4)),
            def("gpu_concurrency", int(Some(1), Some(16)), json!(1)),
            def("per_agent_concurrency", int(Some(1), Some(64)), json!(2)),
            def(
                "per_repository_concurrency",
                int(Some(1), Some(64)),
                json!(1),
            ),
            def("verify_concurrency", int(Some(1), Some(64)), json!(1)),
            def("lease_seconds", int(Some(5), Some(3600)), json!(60)),
            def("default_max_attempts", int(Some(1), Some(50)), json!(5)),
        ],
    }
}

fn stacks() -> Model {
    Model {
        name: "StacksConfig",
        fields: vec![def("auto_accept", Schema::Bool, json!(true))],
    }
}

fn verification() -> Model {
    Model {
        name: "VerificationConfig",
        fields: vec![def("auto_accept", Schema::Bool, json!(true))],
    }
}

fn retention() -> Model {
    Model {
        name: "RetentionConfig",
        fields: vec![
            def("telemetry_payload_days", int(Some(1), None), json!(30)),
            def("run_log_days", int(Some(1), None), json!(90)),
        ],
    }
}

fn claude_integration() -> Model {
    Model {
        name: "ClaudeIntegrationConfig",
        fields: vec![
            def("capture_tool_payloads", Schema::Bool, json!(false)),
            def("max_payload_chars", int(Some(0), None), json!(4000)),
        ],
    }
}

fn automation() -> Model {
    Model {
        name: "AutomationConfig",
        fields: vec![def("max_depth", int(Some(1), Some(10)), json!(3))],
    }
}

fn jira_tracker() -> Model {
    Model {
        name: "JiraTrackerConfig",
        fields: vec![
            def("type", Schema::Literal(&["jira"]), json!("jira")),
            def(
                "flavour",
                Schema::Literal(&["cloud", "data_center"]),
                json!("cloud"),
            ),
            req("base_url", Schema::Str),
            req("workspace", Schema::Str),
            req("projects", Schema::JiraProjects),
            def("email_env", nullable(Schema::Str), Value::Null),
            req("token_env", Schema::Str),
            def("poll_interval_seconds", int(Some(30), None), json!(300)),
            def(
                "full_sync_interval_seconds",
                int(Some(300), None),
                json!(86400),
            ),
            def("overlap_seconds", int(Some(0), None), json!(120)),
            def("missing_grace_seconds", int(Some(0), None), json!(86400)),
            def("pr_fields", list_of(Schema::Str), json!([])),
            def("dev_panel", Schema::Bool, json!(true)),
            def(
                "cancelled_resolutions",
                list_of(Schema::Str),
                json!(["Won't Do", "Duplicate"]),
            ),
            def(
                "timeout_seconds",
                Schema::Float {
                    ge: None,
                    gt: Some(0.0),
                },
                json!(30.0),
            ),
        ],
    }
}

/// `GlobalConfig`: the whole of `config.toml`, schema version 2.
fn global() -> Model {
    Model {
        name: "GlobalConfig",
        fields: vec![
            def("schema_version", int(None, None), json!(2)),
            def("default_workspace", nullable(Schema::Str), Value::Null),
            def("server", model(server()), json!({})),
            def("llm", model(llm()), json!({})),
            def(
                "agents",
                dict_of(agents_union()),
                json!({"claude_code": {"type": "claude_code"}}),
            ),
            def("execution", model(execution()), json!({})),
            def("jobs", model(jobs()), json!({})),
            def("stacks", model(stacks()), json!({})),
            def("verification", model(verification()), json!({})),
            def("retention", model(retention()), json!({})),
            def("claude", model(claude_integration()), json!({})),
            def("automation", model(automation()), json!({})),
            def("trackers", dict_of(model(jira_tracker())), json!({})),
        ],
    }
}

/// `WorkspaceOverrides`: a workspace file. Every section is optional and overrides the global one.
fn overrides() -> Model {
    let section = || nullable(dict_of(Schema::Any));
    Model {
        name: "WorkspaceOverrides",
        fields: vec![
            def("schema_version", int(None, None), json!(2)),
            def("llm", section(), Value::Null),
            def("agents", nullable(dict_of(agents_union())), Value::Null),
            def("execution", section(), Value::Null),
            def("jobs", section(), Value::Null),
            def("stacks", section(), Value::Null),
            def("verification", section(), Value::Null),
        ],
    }
}

/// Validates a whole `config.toml` value. Returns its dump with defaults filled in, or Pydantic's
/// errors.
pub fn validate_global(input: &Value) -> Result<Map<String, Value>, Vec<ValidationError>> {
    validate_root(&global(), input)
}

/// Validates a workspace file value. Returns its dump, with `null` members still present.
pub fn validate_overrides(input: &Value) -> Result<Map<String, Value>, Vec<ValidationError>> {
    validate_root(&overrides(), input)
}

/// The dump of a default `GlobalConfig` (`GlobalConfig().model_dump(mode="json")`).
#[must_use]
pub fn default_global() -> Map<String, Value> {
    validate_root(&global(), &json!({})).unwrap_or_default()
}

/// The dump of a default `WorkspaceOverrides`.
#[must_use]
pub fn default_overrides() -> Map<String, Value> {
    validate_root(&overrides(), &json!({})).unwrap_or_default()
}

fn validate_root(model: &Model, input: &Value) -> Result<Map<String, Value>, Vec<ValidationError>> {
    let mut errors = Vec::new();
    match validate_model(model, input, &[], &mut errors) {
        Some(dump) if errors.is_empty() => Ok(dump),
        _ => Err(errors),
    }
}
