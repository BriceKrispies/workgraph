//! Redaction of secrets and sensitive payloads before anything is persisted (`security/redaction.py`).
//!
//! Patterns are compiled where they are used (a `static` lazily compiled regex is interior mutability,
//! which the Core tier forbids). Each public entry point compiles once and passes the patterns down.

use regex::Regex;
use serde_json::{Map, Value, json};
use wg_types::pyval::prefix;

/// The text that replaces a secret.
pub const REDACTED: &str = "[REDACTED]";
const MAX_DEPTH: usize = 8;
const MAX_KEYS: usize = 200;
const MAX_ITEMS: usize = 100;
const SUMMARY_KEYS: usize = 50;

const SECRET_KEY: &str = r"(?i)(pass(word|wd)?|secret|token|api[_-]?key|auth(orization)?|credential|private[_-]?key|session[_-]?key|cookie)";
const SECRET_VALUES: [&str; 10] = [
    // OpenAI / Anthropic style keys.
    r"sk-[A-Za-z0-9_-]{16,}",
    r"sk-ant-[A-Za-z0-9_-]{16,}",
    // GitHub tokens.
    r"gh[pousr]_[A-Za-z0-9]{20,}",
    r"github_pat_[A-Za-z0-9_]{20,}",
    // AWS access key id.
    r"AKIA[0-9A-Z]{16}",
    // Slack.
    r"xox[baprs]-[A-Za-z0-9-]{10,}",
    r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{12,}",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    r"(?i)(password|passwd|pwd|token|secret|api[_-]?key)\s*[=:]\s*\S+",
    // JWT.
    r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
];

/// The compiled key and value patterns of one redaction. The literals above are compiled by the unit
/// tests, so every pattern here is present in practice; a pattern that failed to compile would simply
/// match nothing rather than panic.
struct Patterns {
    secret_key: Option<Regex>,
    values: Vec<Regex>,
}

fn patterns() -> Patterns {
    Patterns {
        secret_key: Regex::new(SECRET_KEY).ok(),
        values: SECRET_VALUES
            .iter()
            .filter_map(|pattern| Regex::new(pattern).ok())
            .collect(),
    }
}

fn is_secret_key(patterns: &Patterns, key: &str) -> bool {
    patterns
        .secret_key
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(key))
}

/// Replaces every secret-looking substring, then truncates to `max_chars` code points with a marker
/// giving the number of characters cut.
pub fn redact_text(value: &str, max_chars: Option<usize>) -> String {
    redact_with(&patterns(), value, max_chars)
}

fn redact_with(patterns: &Patterns, value: &str, max_chars: Option<usize>) -> String {
    let scrubbed = patterns
        .values
        .iter()
        .fold(value.to_owned(), |text, pattern| {
            pattern.replace_all(&text, REDACTED).into_owned()
        });
    match max_chars {
        Some(limit) if scrubbed.chars().count() > limit => {
            let total = scrubbed.chars().count();
            let head: String = prefix(&scrubbed, limit);
            format!("{head}... [truncated {} chars]", total - limit)
        }
        _ => scrubbed,
    }
}

/// Recursively redacts secret-looking keys and values, bounding size and depth.
pub fn redact(value: &Value, max_chars: usize) -> Value {
    redact_value(&patterns(), value, max_chars, 0)
}

/// [`redact`] applied to an object, returning the object.
pub fn redact_object(map: &Map<String, Value>, max_chars: usize) -> Map<String, Value> {
    redact_map(&patterns(), map, max_chars, 0)
}

fn redact_value(patterns: &Patterns, value: &Value, max_chars: usize, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return json!("[nested too deep]");
    }
    match value {
        Value::Object(map) => Value::Object(redact_map(patterns, map, max_chars, depth)),
        Value::Array(items) => {
            let kept = items
                .iter()
                .take(MAX_ITEMS)
                .map(|item| redact_value(patterns, item, max_chars, depth + 1));
            let overflow = items
                .len()
                .checked_sub(MAX_ITEMS)
                .filter(|extra| *extra > 0)
                .map(|extra| json!(format!("... {extra} more items")));
            Value::Array(kept.chain(overflow).collect())
        }
        Value::String(text) => Value::String(redact_with(patterns, text, Some(max_chars))),
        other => other.clone(),
    }
}

fn redact_map(
    patterns: &Patterns,
    map: &Map<String, Value>,
    max_chars: usize,
    depth: usize,
) -> Map<String, Value> {
    let kept = map.iter().take(MAX_KEYS).map(|(key, item)| {
        let redacted = if is_secret_key(patterns, key) {
            json!(REDACTED)
        } else {
            redact_value(patterns, item, max_chars, depth + 1)
        };
        (key.clone(), redacted)
    });
    let overflow = map
        .len()
        .checked_sub(MAX_KEYS)
        .filter(|extra| *extra > 0)
        .map(|extra| ("...".to_owned(), json!(format!("{extra} more keys"))));
    kept.chain(overflow).collect()
}

/// A shape-only summary (keys, types and sizes) for payloads that must not be stored.
pub fn summarize(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .take(SUMMARY_KEYS)
                .map(|(key, item)| (key.clone(), summarize(item)))
                .collect(),
        ),
        Value::Array(items) => json!({"type": "list", "length": items.len()}),
        Value::String(text) => json!({"type": "str", "length": text.chars().count()}),
        Value::Number(number) if number.is_f64() => json!({"type": "float"}),
        Value::Number(_) => json!({"type": "int"}),
        Value::Bool(_) => json!({"type": "bool"}),
        Value::Null => json!({"type": "NoneType"}),
    }
}
