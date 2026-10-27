//! The OpenAI-compatible HTTP API (`integrations/llm/openai_compatible.py`): `/v1/models`,
//! `/v1/chat/completions` and `/v1/embeddings`. Requests, error mapping and parsing. The client
//! that sends them is in wg-net.

use serde_json::{Map, Value, json};
use wg_model::{HttpCall, HttpMethod};

use crate::error::ProviderError;
use crate::output::{Structured, floats, from_content};
use crate::request::{bearer, call, json_content_type, url};
use crate::text::py_repr;
use wg_types::pyval::prefix;

/// The provider label used in retry messages: the configured name, then the base URL.
pub fn label(name: &str, base_url: &str) -> String {
    format!("{name} at {base_url}")
}

/// `GET /v1/models`.
pub fn models_call(base_url: &str, api_key: Option<&str>, timeout_seconds: f64) -> HttpCall {
    call(
        HttpMethod::Get,
        url(base_url, "/v1/models", &[]),
        bearer(api_key),
        None,
        timeout_seconds,
    )
}

/// `POST /v1/chat/completions` with a strict JSON-Schema response format.
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
    let title = schema
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("result");
    let body = json!({
        "model": model,
        "temperature": temperature,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": title, "schema": schema, "strict": true},
        },
    });
    post(
        base_url,
        "/v1/chat/completions",
        api_key,
        timeout_seconds,
        &body,
    )
}

/// `POST /v1/embeddings` for a batch of texts.
pub fn embed_call(
    base_url: &str,
    api_key: Option<&str>,
    timeout_seconds: f64,
    model: &str,
    texts: &[String],
) -> HttpCall {
    let body = json!({"model": model, "input": texts});
    post(base_url, "/v1/embeddings", api_key, timeout_seconds, &body)
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

/// The model ids in a `/v1/models` body, sorted. An entry without an id reads as `None`.
pub fn model_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .map(|model| match model.get("id") {
                    Some(Value::String(id)) => id.clone(),
                    Some(other) => other.to_string(),
                    None => "None".to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Whether `model` is served, and the sentence `check_model` reports.
pub fn check_model(models: &[String], model: &str) -> (bool, String) {
    if models.iter().any(|served| served == model) {
        (true, format!("{model} available"))
    } else {
        (false, format!("{} not served", py_repr(model)))
    }
}

/// The error for a non-success status, or `None` for a success. The name is the provider's
/// configured name.
pub fn status_error(name: &str, code: u16, body: &str) -> Option<ProviderError> {
    if code == 404 {
        return Some(ProviderError::ModelUnavailable(format!(
            "{name}: {}",
            prefix(body, 200)
        )));
    }
    (code >= 400).then(|| {
        ProviderError::InvalidOutput(format!("{name} error {code}: {}", prefix(body, 300)))
    })
}

/// The structured output of a `/v1/chat/completions` body: `choices[0].message.content` must be
/// a JSON object. Any payload that does not have that shape is an unexpected payload.
pub fn parse_chat(body: &Value, request_model: &str) -> Result<Structured, ProviderError> {
    let unexpected = || {
        ProviderError::InvalidOutput(format!(
            "unexpected completion payload: {}",
            prefix(&body.to_string(), 200)
        ))
    };
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(unexpected)?;
    serde_json::from_str::<Value>(content).map_err(|_| unexpected())?;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .unwrap_or(request_model)
        .to_owned();
    from_content(content, model, Map::new())
}

/// The vectors of a `/v1/embeddings` body, ordered by each item's `index`.
pub fn parse_embeddings(body: &Value, expected: usize) -> Result<Vec<Vec<f64>>, ProviderError> {
    let mismatch = || ProviderError::InvalidOutput("embedding count mismatch".to_owned());
    let mut items: Vec<&Value> = body
        .get("data")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    if items.len() != expected {
        return Err(mismatch());
    }
    items.sort_by_key(|item| item.get("index").and_then(Value::as_u64).unwrap_or(0));
    items
        .into_iter()
        .map(|item| item.get("embedding").and_then(floats).ok_or_else(mismatch))
        .collect()
}
