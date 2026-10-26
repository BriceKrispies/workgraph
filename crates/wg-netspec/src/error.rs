//! The errors the provider clients and the Jira client report. Their messages never carry
//! credentials: a request's headers are not part of any message.

/// A failure of an inference provider (`ProviderUnavailableError`, `ModelUnavailableError`,
/// `InvalidModelOutputError` in `integrations/llm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider cannot be reached, or kept answering with a retryable status.
    Unavailable(String),
    /// The provider answered that the model is not served or pulled.
    ModelUnavailable(String),
    /// The provider answered, but not with usable output.
    InvalidOutput(String),
}

impl ProviderError {
    /// The message, without the variant.
    pub fn message(&self) -> &str {
        match self {
            Self::Unavailable(message)
            | Self::ModelUnavailable(message)
            | Self::InvalidOutput(message) => message,
        }
    }
}

/// A Jira request failed. The message names the path and the status, never credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerError {
    pub message: String,
    /// The HTTP status, when the server answered with one.
    pub status: Option<u16>,
}
