//! The endpoint settings of a provider and the result of a structured generation. The clients
//! that send requests with these settings are in wg-net; the retry decisions they follow are in
//! [`crate::retry`].

use serde_json::{Map, Value};

use crate::output::Structured;
use crate::retry::Backoff;

/// Where a provider lives and how its calls behave (`OllamaProviderConfig` and
/// `OpenAICompatibleProviderConfig`). The API key is read from the environment by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout_seconds: f64,
    pub max_retries: u32,
    pub backoff_seconds: f64,
}

impl Endpoint {
    /// The endpoint of a dumped `llm.providers` entry (`OllamaProviderConfig` or
    /// `OpenAICompatibleProviderConfig`). Absent settings take the model's defaults; the base URL
    /// defaults to `default_base_url`. The API key is the caller's, read from its environment.
    #[must_use]
    pub fn from_config(
        entry: &Map<String, Value>,
        default_base_url: &str,
        api_key: Option<String>,
    ) -> Self {
        let number =
            |key: &str, default: f64| entry.get(key).and_then(Value::as_f64).unwrap_or(default);
        Self {
            base_url: entry
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or(default_base_url)
                .to_owned(),
            api_key,
            timeout_seconds: number("timeout_seconds", 120.0),
            max_retries: u32::try_from(
                entry
                    .get("max_retries")
                    .and_then(Value::as_u64)
                    .unwrap_or(2),
            )
            .unwrap_or(u32::MAX),
            backoff_seconds: number("backoff_seconds", 1.0),
        }
    }

    /// The backoff of a call made with this endpoint's retries.
    pub fn backoff(&self) -> Backoff {
        Backoff {
            max_retries: self.max_retries,
            base_seconds: self.backoff_seconds,
        }
    }
}

/// A structured generation with what the call cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    pub output: Structured,
    /// Attempts made, retries included.
    pub attempts: u32,
    pub duration_ms: u64,
}
