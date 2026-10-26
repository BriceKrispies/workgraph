//! wg-telemetryspec: the Claude Code hook payload model (`HookPayload` in `integrations/claude_code/ingest.py`),
//! validated the way Pydantic validates it.
//!
//! The shell validates each hook object with [`validate_hook_payload`] before it calls
//! `wg_telemetry::hook::normalise`, which is Core and so cannot depend on `wg-validate`. The validated
//! object is `HookPayload.model_dump(mode="json")` over the declared fields. Pydantic's `extra="allow"`
//! keeps unknown keys; [`Extra::Ignore`] here drops them, and the shell keeps them from the raw object.

use serde_json::{Map, Value};
use wg_validate::{
    Extra, Field, IntRules, Loc, Model, Schema, StrRules, ValidationError, validate,
};

/// The model of the common hook fields, in Pydantic's declaration order (which sets the error order).
pub fn hook_payload_model() -> Model {
    Model::new(
        "HookPayload",
        Extra::Ignore,
        vec![
            Field::required("hook_event_name", string()),
            optional("session_id", nullable_string()),
            optional("transcript_path", nullable_string()),
            optional("cwd", nullable_string()),
            optional("permission_mode", nullable_string()),
            optional("prompt_id", nullable_string()),
            optional("agent_id", nullable_string()),
            optional("agent_type", nullable_string()),
            optional("source", nullable_string()),
            optional("reason", nullable_string()),
            optional("model", Schema::Any),
            optional("tool_name", nullable_string()),
            optional("tool_use_id", nullable_string()),
            optional("tool_input", Schema::Any),
            optional("tool_response", Schema::Any),
            optional("error", nullable_string()),
            optional("is_interrupt", nullable_bool()),
            optional("duration_ms", nullable_int()),
            optional("agent_transcript_path", nullable_string()),
            optional("last_assistant_message", nullable_string()),
            optional("stop_hook_active", nullable_bool()),
            optional("task_id", nullable_string()),
            optional("task_subject", nullable_string()),
            optional("task_description", nullable_string()),
            optional("teammate_name", nullable_string()),
        ],
    )
}

/// Validates a hook object (a JSON object with `hook_event_name`) in Pydantic's lax mode. On success the
/// validated declared fields; otherwise Pydantic's error list.
pub fn validate_hook_payload(raw: &Map<String, Value>) -> Result<Value, Vec<ValidationError>> {
    validate(&hook_payload_model(), &Value::Object(raw.clone()))
}

/// The text of a failed validation, in the shape of Pydantic's `ValidationError` message: the count, then each
/// error as its location, its message and its type.
pub fn failure_message(errors: &[ValidationError]) -> String {
    let count = errors.len();
    let noun = if count == 1 { "error" } else { "errors" };
    let items: Vec<String> = errors
        .iter()
        .map(|error| {
            let loc: Vec<String> = error
                .loc
                .iter()
                .map(|step| match step {
                    Loc::Key(key) => key.clone(),
                    Loc::Index(index) => index.to_string(),
                })
                .collect();
            format!("{}\n  {} [type={}]", loc.join("."), error.msg, error.kind)
        })
        .collect();
    format!(
        "{count} validation {noun} for HookPayload\n{}",
        items.join("\n")
    )
}

fn optional(name: &str, schema: Schema) -> Field {
    Field::optional(name, schema, Value::Null)
}

fn string() -> Schema {
    Schema::Str(StrRules::default())
}

fn nullable_string() -> Schema {
    Schema::Nullable(Box::new(string()))
}

fn nullable_bool() -> Schema {
    Schema::Nullable(Box::new(Schema::Bool))
}

fn nullable_int() -> Schema {
    Schema::Nullable(Box::new(Schema::Int(IntRules::default())))
}
