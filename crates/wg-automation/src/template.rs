//! `{{path}}` placeholders in automation text (`render` in `automation/rules.py`).
//!
//! Values are data, never code: a substituted value is not expanded again. A missing or null
//! path renders as the empty string.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

use crate::condition::resolve;
use crate::py::str_of;

/// Longest substituted value kept, in characters, unless the caller gives another limit.
pub const DEFAULT_LIMIT: usize = 500;

const PLACEHOLDER_PATTERN: &str = r"\{\{\s*([a-z_][a-z0-9_.]*)\s*\}\}";

static PLACEHOLDER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(PLACEHOLDER_PATTERN).ok());

/// Substitutes every placeholder of `template` with the value at its path in `context`, each
/// value cut to `limit` characters.
pub fn render(template: &str, context: &Value, limit: usize) -> String {
    let Some(placeholder) = PLACEHOLDER.as_ref() else {
        return template.to_owned();
    };
    placeholder
        .replace_all(template, |captures: &Captures<'_>| {
            match resolve(context, &captures[1]) {
                None | Some(Value::Null) => String::new(),
                Some(value) => str_of(value).chars().take(limit).collect(),
            }
        })
        .into_owned()
}

/// [`render`] with the default limit of [`DEFAULT_LIMIT`].
pub fn render_default(template: &str, context: &Value) -> String {
    render(template, context, DEFAULT_LIMIT)
}
