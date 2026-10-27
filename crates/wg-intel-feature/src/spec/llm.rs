//! The pure side of the inference providers: the outcome of a provider call and how its errors map
//! to the workgraph errors the services raise or store, and the `(provider, model)` selection of a
//! configuration. The clients that make the calls are in [`crate::service::llm`].

use serde_json::{Map, Value as Json};
use wg_netspec::{ProviderError, Structured};
use wg_registry::providers::llm_selection;
use wg_types::WgError;

/// What a provider call failed with. The services map each case as Python's exception handling
/// does: an unavailable provider and invalid output are stored or raised by the caller.
#[derive(Debug)]
pub enum CallError {
    /// The provider's own error (`ProviderUnavailableError`, `InvalidModelOutputError`).
    Provider(ProviderError),
    /// The provider cannot produce the output asked for (`CapabilityNotSupportedError`).
    Unsupported(String),
    /// Any other failure, which the caller raises as it is.
    Raised(WgError),
}

/// A structured answer: the JSON object and the time the call took.
#[derive(Debug, Clone, PartialEq)]
pub struct Generation {
    pub data: Map<String, Json>,
    pub duration_ms: u64,
}

/// The generation of a provider's structured output, with its duration.
#[must_use]
pub fn generation_of(output: Structured, duration_ms: u64) -> Generation {
    Generation {
        data: output.data,
        duration_ms,
    }
}

/// Whether a provider error means the provider is down or lacks the model (Python's
/// `ProviderUnavailableError` and its `ModelUnavailableError` subclass).
#[must_use]
pub fn is_unavailable(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unavailable(_) | ProviderError::ModelUnavailable(_)
    )
}

/// The provider error as the `WgError` the caller raises when the provider is unavailable.
#[must_use]
pub fn unavailable_error(error: &ProviderError) -> WgError {
    WgError::provider_unavailable(error.message())
}

/// The error of an embedding call that failed: an unavailable provider is raised as such, any
/// other failure as a workgraph error.
#[must_use]
pub fn embedding_call_error(error: CallError) -> WgError {
    match error {
        CallError::Provider(error) if is_unavailable(&error) => unavailable_error(&error),
        CallError::Provider(error) => WgError::workgraph(error.message()),
        CallError::Unsupported(message) => WgError::workgraph(message),
        CallError::Raised(error) => error,
    }
}

/// The `(provider, model)` of the `capability` selection (`llm.classification`,
/// `llm.embeddings`, ...) of a dumped configuration (`LLMRegistry.selection`).
pub fn selection(
    config: &Map<String, Json>,
    capability: &str,
) -> Result<(String, String), WgError> {
    llm_selection(config, capability)
        .map(|(provider, model)| (provider.to_owned(), model.to_owned()))
        .ok_or_else(|| WgError::validation(format!("llm.{capability} is not set")))
}
