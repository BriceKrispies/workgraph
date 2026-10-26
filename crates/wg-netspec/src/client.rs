//! The endpoint settings of a provider and the result of a structured generation. The clients
//! that send requests with these settings are in wg-net; the retry decisions they follow are in
//! [`crate::retry`].

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
