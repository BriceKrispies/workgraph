//! The settings documents of the API (`api/routes/ops.py`): which configuration keys `PATCH
//! /api/v1/settings` may change and how a patch merges into the loaded configuration.
//!
//! The shell loads `config.toml`, calls [`patched_config`], and writes the result. The rules here
//! are the ones of the Python route; the validation is `wg_config::validate_global`.

use serde_json::{Map, Value};
use wg_config::validate_global;
use wg_runtimespec::app::configured_default_workspace;
use wg_types::WgError;
use wg_validate::pydantic::error_text;

/// The dotted-key prefixes a settings patch may change (`EDITABLE_PREFIXES`). A key is editable
/// when it starts with one of them.
pub const EDITABLE_PREFIXES: [&str; 12] = [
    "execution.",
    "jobs.",
    "llm.classification.",
    "llm.embeddings.",
    "llm.summarization.",
    "llm.auto_classify",
    "llm.auto_embed",
    "retention.",
    "claude.capture_tool_payloads",
    "automation.max_depth",
    "stacks.auto_accept",
    "verification.auto_accept",
];

/// Applies a settings patch to the loaded configuration and validates the result.
///
/// Each key is a dotted path. The first key outside [`EDITABLE_PREFIXES`] is refused with
/// `validation_failed`; the rest are written in order, creating intermediate tables as
/// `setdefault` does. The merged configuration must validate as a whole; its validated dump, with
/// defaults filled in, is returned.
pub fn patched_config(
    current: &Map<String, Value>,
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, WgError> {
    let mut data = current.clone();
    for (key, value) in values {
        if !EDITABLE_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
        {
            return Err(WgError::validation(format!(
                "{key} cannot be edited from the API; use the config file"
            )));
        }
        set_dotted(&mut data, key, value.clone())?;
    }
    validate_global(&Value::Object(data)).map_err(|errors| {
        WgError::validation(format!(
            "invalid settings: {}",
            error_text("GlobalConfig", &errors)
        ))
    })
}

/// Sets `key` (dotted) in `table` to `value`, creating the tables on the way.
fn set_dotted(table: &mut Map<String, Value>, key: &str, value: Value) -> Result<(), WgError> {
    let parts: Vec<&str> = key.split('.').collect();
    let Some((last, path)) = parts.split_last() else {
        return Ok(());
    };
    let mut cursor = table;
    for part in path {
        let entry = cursor
            .entry((*part).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        cursor = entry.as_object_mut().ok_or_else(|| {
            WgError::workgraph(format!("cannot set {key}: {part} is not a table"))
        })?;
    }
    cursor.insert((*last).to_owned(), value);
    Ok(())
}

/// The workspace whose settings `GET /api/v1/settings` reads: the `workspace` query value when it
/// is a non-empty string, else the configured default (`workspace or app.config.default_workspace`).
pub fn settings_workspace(
    params: &Map<String, Value>,
    config: &Map<String, Value>,
) -> Option<String> {
    params
        .get("workspace")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map(str::to_owned)
        .or_else(|| configured_default_workspace(config))
}

/// The `values` of a `SettingsPatch` body (validated against the snapshot before this point).
pub fn patch_values(body: &Value) -> Map<String, Value> {
    body.get("values")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wg_config::default_global;

    use super::*;
    use wg_types::ErrorCode;

    fn values(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn editable_keys_are_merged_and_validated() {
        let current = default_global();
        let next = patched_config(
            &current,
            &values(json!({"execution.max_concurrent_runs": 3})),
        )
        .unwrap_or_default();
        assert_eq!(next["execution"]["max_concurrent_runs"], json!(3));
        assert_eq!(current["execution"]["max_concurrent_runs"], json!(2));
    }

    #[test]
    fn keys_outside_the_editable_prefixes_are_refused_first() {
        let current = default_global();
        let error = patched_config(&current, &values(json!({"server.port": 1})));
        let error = error.err();
        assert_eq!(
            error.as_ref().map(|e| e.code),
            Some(ErrorCode::ValidationFailed)
        );
        assert_eq!(
            error.map(|e| e.message),
            Some("server.port cannot be edited from the API; use the config file".to_owned())
        );
    }

    #[test]
    fn invalid_values_fail_validation_with_details() {
        let current = default_global();
        let error = patched_config(
            &current,
            &values(json!({"execution.max_concurrent_runs": 0})),
        )
        .err();
        assert_eq!(
            error.as_ref().map(|e| e.code),
            Some(ErrorCode::ValidationFailed)
        );
        let message = error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        assert!(
            message.starts_with("invalid settings: 1 validation error for GlobalConfig\n"),
            "{message}"
        );
        assert!(
            message.contains("execution.max_concurrent_runs"),
            "{message}"
        );
        assert!(error.is_some_and(|e| e.details.is_empty()));
    }
}
