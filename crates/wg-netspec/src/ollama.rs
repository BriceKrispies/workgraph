//! The Ollama HTTP API (`integrations/llm/ollama.py`): `/api/tags`, `/api/chat` and `/api/embed`.
//! Requests, model matching, error mapping and parsing. The client that sends them is in wg-net.

use serde_json::{Map, Value, json};
use wg_model::{HttpCall, HttpMethod};

use crate::error::ProviderError;
use crate::output::{Structured, floats, from_content};
use crate::request::{bearer, call, json_content_type, url};
use crate::text::py_repr;
use wg_types::pyval::prefix;

/// The provider label used in messages.
pub fn label(base_url: &str) -> String {
    format!("ollama at {base_url}")
}

/// `GET /api/tags`: the models the server has.
pub fn tags_call(base_url: &str, api_key: Option<&str>, timeout_seconds: f64) -> HttpCall {
    call(
        HttpMethod::Get,
        url(base_url, "/api/tags", &[]),
        bearer(api_key),
        None,
        timeout_seconds,
    )
}

/// `POST /api/chat` with a JSON-Schema constrained format and no streaming.
#[allow(clippy::too_many_arguments)]
pub fn chat_call(
    base_url: &str,
    api_key: Option<&str>,
    timeout_seconds: f64,
    model: &str,
    system: &str,
    prompt: &str,
    schema: &Value,
    temperature: f64,
) -> HttpCall {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
        ],
        "format": schema,
        "stream": false,
        "options": {"temperature": temperature},
    });
    post(base_url, "/api/chat", api_key, timeout_seconds, &body)
}

/// `POST /api/embed` for a batch of texts.
pub fn embed_call(
    base_url: &str,
    api_key: Option<&str>,
    timeout_seconds: f64,
    model: &str,
    texts: &[String],
) -> HttpCall {
    let body = json!({"model": model, "input": texts});
    post(base_url, "/api/embed", api_key, timeout_seconds, &body)
}

fn post(
    base_url: &str,
    path: &str,
    api_key: Option<&str>,
    timeout_seconds: f64,
    body: &Value,
) -> HttpCall {
    let mut headers = bearer(api_key);
    headers.push(json_content_type());
    call(
        HttpMethod::Post,
        url(base_url, path, &[]),
        headers,
        Some(body),
        timeout_seconds,
    )
}

/// The model names in a `/api/tags` body, sorted.
pub fn model_names(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body
        .get("models")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Whether `model` is among `models`, and the sentence `check_model` reports. A name matches with
/// or without its `:latest` tag.
pub fn check_model(models: &[String], model: &str) -> (bool, String) {
    let found = models.iter().any(|name| {
        name == model
            || name.strip_suffix(":latest") == Some(model)
            || *name == format!("{model}:latest")
    });
    if found {
        return (true, format!("{model} available"));
    }
    let available: Vec<&str> = models.iter().take(10).map(String::as_str).collect();
    let listing = if available.is_empty() {
        "none".to_owned()
    } else {
        available.join(", ")
    };
    (
        false,
        format!(
            "model {} is not pulled (available: {listing}); run `ollama pull {model}`",
            py_repr(model)
        ),
    )
}

/// The error for a non-retryable status that is not a success, or `None` for a success.
pub fn status_error(model: &str, code: u16, body: &str) -> Option<ProviderError> {
    if code == 404 {
        let message = error_message(body);
        let reason = if message.is_empty() {
            "not found".to_owned()
        } else {
            message
        };
        return Some(ProviderError::ModelUnavailable(format!(
            "ollama model {} is unavailable: {reason}",
            py_repr(model)
        )));
    }
    (code >= 400).then(|| {
        ProviderError::InvalidOutput(format!("ollama error {code}: {}", prefix(body, 300)))
    })
}

/// The `error` field of a JSON error body, or the empty string.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The structured output of a `/api/chat` body. The message content is the JSON object.
pub fn parse_chat(body: &Value, request_model: &str) -> Result<Structured, ProviderError> {
    let content = body
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .unwrap_or(request_model)
        .to_owned();
    let mut metadata = Map::new();
    for key in ["total_duration", "eval_count", "prompt_eval_count"] {
        if let Some(value) = body.get(key) {
            metadata.insert(key.to_owned(), value.clone());
        }
    }
    from_content(content, model, metadata)
}

/// The vectors of a `/api/embed` body, one per text, in order.
pub fn parse_embeddings(body: &Value, expected: usize) -> Result<Vec<Vec<f64>>, ProviderError> {
    let unexpected = || {
        ProviderError::InvalidOutput("ollama returned an unexpected embedding payload".to_owned())
    };
    let vectors = body
        .get("embeddings")
        .and_then(Value::as_array)
        .filter(|vectors| vectors.len() == expected)
        .ok_or_else(unexpected)?;
    vectors
        .iter()
        .map(|vector| floats(vector).ok_or_else(unexpected))
        .collect()
}
