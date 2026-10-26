//! Request building: the URL with its query string, the headers and the [`HttpCall`] for each
//! request. Pure, so a call can be checked without a socket.

use rpds::VectorSync;
use serde_json::Value;
use wg_model::{HttpCall, HttpMethod};

use crate::text::percent_encode;

/// A header, as `(name, value)`.
pub type Header = (String, String);

/// Query parameters as `(name, value)`, in the order sent.
pub type Query = Vec<(String, String)>;

/// The URL for `path` under `base` (a trailing slash on `base` is dropped), with `query` encoded.
pub fn url(base: &str, path: &str, query: &[(String, String)]) -> String {
    let mut url = format!("{}{}", base.trim_end_matches('/'), path);
    if !query.is_empty() {
        let pairs: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
            .collect();
        url.push('?');
        url.push_str(&pairs.join("&"));
    }
    url
}

/// `Authorization: Bearer <key>`, when there is a key.
pub fn bearer(api_key: Option<&str>) -> Vec<Header> {
    match api_key.filter(|key| !key.is_empty()) {
        Some(key) => vec![("Authorization".to_owned(), format!("Bearer {key}"))],
        None => Vec::new(),
    }
}

/// The `Content-Type` of a JSON request body.
pub fn json_content_type() -> Header {
    ("Content-Type".to_owned(), "application/json".to_owned())
}

/// One call. A JSON body is serialised into the call; a timeout in seconds becomes milliseconds.
pub fn call(
    method: HttpMethod,
    url: String,
    headers: Vec<Header>,
    body: Option<&Value>,
    timeout_seconds: f64,
) -> HttpCall {
    HttpCall {
        method,
        url,
        headers: headers.into_iter().collect::<VectorSync<Header>>(),
        body: body.map(Value::to_string),
        timeout_ms: Some(seconds_to_millis(timeout_seconds)),
    }
}

/// A response body as JSON; a body that is not JSON reads as `null`, so every lookup misses.
pub fn parse_body(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

/// `seconds` as whole milliseconds; a negative or non-finite value counts as zero.
pub fn seconds_to_millis(seconds: f64) -> u64 {
    let millis = (seconds * 1000.0).round();
    if millis.is_finite() && millis > 0.0 {
        // Float-to-integer `as` saturates: an absurd timeout simply means "very long".
        millis as u64
    } else {
        0
    }
}
