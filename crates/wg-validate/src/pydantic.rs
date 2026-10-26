//! Pydantic's `str(ValidationError)`: the text the Python API and SDK put in a validation message.
//!
//! ```text
//! 1 validation error for task_getArguments
//! task
//!   Input should be a valid string [type=string_type, input_value=5, input_type=int]
//!     For further information visit https://errors.pydantic.dev/2.14/v/string_type
//! ```

use serde_json::Value;
use wg_json::py_float_repr;

use crate::error::{Loc, ValidationError};

/// The Pydantic version whose documentation links the error text carries (the recorded one).
const PYDANTIC_DOCS: &str = "https://errors.pydantic.dev/2.14/v";

/// The longest `input_value` repr Pydantic prints before it elides the middle.
const REPR_LIMIT: usize = 50;

/// `str(ValidationError)` for a model named `title` with these errors.
pub fn error_text(title: &str, errors: &[ValidationError]) -> String {
    let count = errors.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut out = format!("{count} validation error{plural} for {title}");
    for error in errors {
        let loc: Vec<String> = error
            .loc
            .iter()
            .map(|step| match step {
                Loc::Key(key) => key.clone(),
                Loc::Index(index) => index.to_string(),
            })
            .collect();
        if !loc.is_empty() {
            out.push('\n');
            out.push_str(&loc.join("."));
        }
        out.push_str(&format!(
            "\n  {} [type={}, input_value={}, input_type={}]",
            error.msg,
            error.kind,
            truncated_repr(&error.input),
            type_name(&error.input),
        ));
        out.push_str(&format!(
            "\n    For further information visit {}",
            error_url(&error.kind)
        ));
    }
    out
}

/// The documentation link Pydantic attaches to an error of this type: the `url` key of
/// `ValidationError.errors()`.
pub fn error_url(kind: &str) -> String {
    format!("{PYDANTIC_DOCS}/{kind}")
}

/// Python's type name of a JSON value, as `input_type=` shows it.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Pydantic's `truncate_safe_repr`: a repr longer than 50 UTF-8 bytes keeps a head of at most 25
/// bytes and a tail of at most 24, each cut at a character boundary, joined by `...`. Checked
/// against Pydantic 2.14 for ASCII, two-, three- and four-byte characters (`golden/`).
fn truncated_repr(value: &Value) -> String {
    let repr = py_repr(value);
    if repr.len() <= REPR_LIMIT {
        return repr;
    }
    let mut head_end = REPR_LIMIT / 2;
    while !repr.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = repr.len() - (REPR_LIMIT / 2 - 1);
    while !repr.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!("{}...{}", &repr[..head_end], &repr[tail_start..])
}

/// `repr()` of the Python value a JSON value decodes to.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => match (n.is_f64(), n.as_f64()) {
            (true, Some(float)) => py_float_repr(float),
            _ => n.to_string(),
        },
        Value::String(text) => str_repr(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}: {}", str_repr(key), py_repr(item)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// `repr(str)`: single quotes unless the text has a single quote and no double quote.
fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repr_follows_python() {
        assert_eq!(
            py_repr(&json!({"a": [1, 2.5, null, true], "b": "it's"})),
            "{'a': [1, 2.5, None, True], 'b': \"it's\"}"
        );
    }

    #[test]
    fn long_repr_is_elided() {
        let long = Value::String("a".repeat(80));
        let text = truncated_repr(&long);
        assert_eq!(text.chars().count(), 25 + 3 + 24);
        // 48 characters make a 50-byte repr with the quotes: kept whole.
        let edge = Value::String("a".repeat(48));
        assert_eq!(truncated_repr(&edge).len(), 50);
        // Multi-byte characters count in bytes: 25 two-byte characters are too long.
        let accented = Value::String("é".repeat(25));
        assert_eq!(truncated_repr(&accented).matches("...").count(), 1);
    }
}
