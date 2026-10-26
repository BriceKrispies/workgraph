//! The result of a structured generation, as `StructuredResponse` carries it.

use serde_json::{Map, Value};

use crate::error::ProviderError;
use crate::text::{head, py_repr};

/// A JSON object the model produced, with the raw text and the provider's metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Structured {
    pub data: Map<String, Value>,
    pub raw_text: String,
    /// The model the provider reports, or the one requested when it reports none.
    pub model: String,
    /// Provider counters (`total_duration`, `eval_count`, `prompt_eval_count`) that were present.
    pub metadata: Map<String, Value>,
}

/// The structured output held in a provider's message `content`: it must be a JSON object.
pub fn from_content(
    content: &str,
    model: String,
    metadata: Map<String, Value>,
) -> Result<Structured, ProviderError> {
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(data)) => Ok(Structured {
            data,
            raw_text: content.to_owned(),
            model,
            metadata,
        }),
        Ok(_) => Err(ProviderError::InvalidOutput(
            "model output is not a JSON object".to_owned(),
        )),
        Err(_) => Err(ProviderError::InvalidOutput(format!(
            "model returned non-JSON output: {}",
            py_repr(&head(content, 200))
        ))),
    }
}

/// An array of JSON numbers as floats; `None` when the value is not such an array.
pub fn floats(value: &Value) -> Option<Vec<f64>> {
    value.as_array()?.iter().map(Value::as_f64).collect()
}
