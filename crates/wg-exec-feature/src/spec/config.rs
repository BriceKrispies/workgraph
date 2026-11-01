//! What the run flow reads from a dumped configuration, and how a provider failure is reported.

use serde_json::{Map, Value};
use wg_netspec::ProviderError;
use wg_types::{ErrorCode, WgError};

/// The member of a dumped configuration at `path`.
pub fn config_at<'a>(config: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    rest.iter()
        .try_fold(config.get(*first)?, |value, key| value.get(*key))
}

/// `execution.enabled`.
pub fn execution_enabled(config: &Map<String, Value>) -> bool {
    config_at(config, &["execution", "enabled"])
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A boolean `execution.<key>`, `false` when unset.
pub fn execution_flag(config: &Map<String, Value>, key: &str) -> bool {
    config_at(config, &["execution", key])
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The integer `execution.<key>`, or `default`.
pub fn execution_number(config: &Map<String, Value>, key: &str, default: i64) -> i64 {
    config_at(config, &["execution", key])
        .and_then(Value::as_i64)
        .unwrap_or(default)
}

/// The text `execution.<key>`, or `default`.
pub fn execution_text(config: &Map<String, Value>, key: &str, default: &str) -> String {
    config_at(config, &["execution", key])
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

/// The error a provider failure is raised as: an unreachable provider or model is unavailable, an
/// answer that is not usable is not valid.
pub fn provider_failure(error: &ProviderError) -> WgError {
    let code = match error {
        ProviderError::Unavailable(_) | ProviderError::ModelUnavailable(_) => {
            ErrorCode::ProviderUnavailable
        }
        ProviderError::InvalidOutput(_) => ErrorCode::ValidationFailed,
    };
    let raised = WgError::new(code, error.message());
    match error.unreachable_details() {
        Some((provider, attempts)) => raised
            .with_detail("provider", Value::from(provider))
            .with_detail("attempts", Value::from(attempts)),
        None => raised,
    }
}
