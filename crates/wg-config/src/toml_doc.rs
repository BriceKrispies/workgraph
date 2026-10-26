//! Parsing a TOML document into the JSON-shaped value the model validates.
//!
//! Key order is kept (the `toml` crate's `preserve_order` feature), so extra keys are reported in
//! document order as Pydantic reports them. Invalid TOML is reported in the words and positions of
//! Python's `tomllib`, which the Python `ConfigStore` raises.

use serde_json::{Map, Value};

use crate::error::ConfigError;

/// Parses a configuration document. Invalid TOML is [`ConfigError::InvalidToml`], carrying the
/// `tomllib` detail (see [`tomllib_detail`]).
pub fn parse_document(text: &str) -> Result<Map<String, Value>, ConfigError> {
    let table: toml::Table = toml::from_str(text)
        .map_err(|error| ConfigError::InvalidToml(tomllib_detail(text, &error)))?;
    Ok(table
        .into_iter()
        .map(|(key, value)| (key, from_toml(value)))
        .collect())
}

fn from_toml(value: toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text),
        toml::Value::Integer(int) => Value::from(int),
        // A non-finite float has no JSON form; it becomes null and fails validation as a number.
        toml::Value::Float(float) => Value::from(float),
        toml::Value::Boolean(flag) => Value::Bool(flag),
        toml::Value::Datetime(datetime) => Value::String(datetime.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(from_toml).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, from_toml(value)))
                .collect(),
        ),
    }
}

/// The detail `tomllib.TOMLDecodeError` carries for a document the `toml` crate rejects: tomllib's
/// wording for the failure, then `(at line L, column C)` or `(at end of document)`.
pub(crate) fn tomllib_detail(text: &str, error: &toml::de::Error) -> String {
    let start = error.span().map_or(text.len(), |span| span.start);
    tomllib_message(text, error.message(), start)
}

/// `tomllib`'s message for a failure the `toml` crate describes as `message` at byte `start`.
///
/// The `toml` crate and tomllib agree on where a document goes wrong in most cases but word the
/// failure differently, so each `toml` crate kind maps to the tomllib message the same failure
/// raises. A kind with no mapping falls back to `Invalid statement`, the tomllib message for a
/// line that starts no statement. Positions follow tomllib's: the first non-blank character at or
/// after the failure, and for an unclosed array the start of the element that is not a value.
fn tomllib_message(text: &str, message: &str, start: usize) -> String {
    let (wording, at) = classify(text, message, start);
    format!("{wording} (at {})", coordinates(text, at))
}

/// The tomllib wording for a `toml` crate message, and the byte index tomllib reports it at.
fn classify(text: &str, message: &str, start: usize) -> (&'static str, usize) {
    let at = skip_spaces(text, start);
    if message.starts_with("unclosed array table") {
        (
            "Expected ']]' at the end of an array declaration",
            last_token_end(text, start),
        )
    } else if message.starts_with("unclosed table") {
        ("Expected ']' at the end of a table declaration", at)
    } else if message.starts_with("unclosed inline table") {
        ("Unclosed inline table", at)
    } else if message.contains("expected `=`") {
        ("Expected '=' after a key in a key/value pair", at)
    } else if message.starts_with("unexpected key or value")
        || message.starts_with("unexpected content")
    {
        (
            "Expected newline or end of document after a statement",
            token_before(text, start),
        )
    } else if message.starts_with("unclosed array")
        || (inside_array(text, start) && is_value_failure(message))
    {
        unclosed_array(text, start)
    } else if is_value_failure(message) {
        ("Invalid value", at)
    } else {
        ("Invalid statement", at)
    }
}

/// Message fragments the `toml` crate uses for a value that does not parse.
const VALUE_FAILURES: [&str; 16] = [
    "invalid value",
    "missing value",
    "string values must be quoted",
    "invalid string",
    "invalid basic string",
    "invalid literal string",
    "invalid multi-line",
    "invalid boolean",
    "invalid float",
    "invalid date-time",
    "invalid mantissa",
    "invalid fraction",
    "invalid exponent",
    "unexpected leading zero",
    "unexpected sign",
    "missing escaped value",
];

fn is_value_failure(message: &str) -> bool {
    VALUE_FAILURES.iter().any(|kind| message.contains(kind))
}

/// Whether byte `start` lies inside an array: the last square bracket before it opens one.
fn inside_array(text: &str, start: usize) -> bool {
    text.get(..start).is_some_and(|before| {
        before
            .rfind(['[', ']'])
            .is_some_and(|at| before.as_bytes()[at] == b'[')
    })
}

/// An array tomllib fails in. It fails at the first element that is not a value, or else just after
/// the first value that is followed by something other than a comma or the closing bracket.
fn unclosed_array(text: &str, start: usize) -> (&'static str, usize) {
    let element = text
        .get(..start)
        .and_then(|before| before.rfind(['[', ',']))
        .map_or(start, |open| skip_blanks(text, open + 1));
    let rest = text.get(element..).unwrap_or_default();
    if starts_value(rest) {
        let value_end = element
            + rest
                .find(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ']' | '#'))
                .unwrap_or(rest.len());
        ("Unclosed array", skip_blanks(text, value_end))
    } else {
        ("Invalid value", element)
    }
}

/// The start of the token that ends before `start`, where tomllib reports a trailing token.
fn token_before(text: &str, start: usize) -> usize {
    text.get(..start)
        .map(|before| before.trim_end_matches([' ', '\t']))
        .and_then(|before| before.rfind(char::is_whitespace).map(|at| at + 1))
        .unwrap_or(0)
}

/// The index of the last character before `start` that is not a space or a tab.
fn last_token_end(text: &str, start: usize) -> usize {
    text.get(..start)
        .map(|before| before.trim_end_matches([' ', '\t']))
        .and_then(|before| before.char_indices().last().map(|(at, _)| at))
        .unwrap_or(0)
}

/// Whether a value can begin at the start of `rest`.
fn starts_value(rest: &str) -> bool {
    rest.starts_with(['"', '\'', '[', '{', '+', '-'])
        || rest.starts_with(|ch: char| ch.is_ascii_digit())
        || ["true", "false", "inf", "nan"]
            .iter()
            .any(|word| rest.starts_with(word))
}

/// The index of the first character at or after `index` that is not a space or a tab.
fn skip_spaces(text: &str, index: usize) -> usize {
    skip_while(text, index, |ch| ch == ' ' || ch == '\t')
}

/// The index of the first character at or after `index` that is not whitespace, including
/// newlines, which an array allows between its elements.
fn skip_blanks(text: &str, index: usize) -> usize {
    skip_while(text, index, char::is_whitespace)
}

fn skip_while(text: &str, index: usize, blank: impl Fn(char) -> bool) -> usize {
    text.get(index..)
        .and_then(|rest| rest.char_indices().find(|(_, ch)| !blank(*ch)))
        .map_or(text.len(), |(offset, _)| index + offset)
}

/// tomllib's coordinates for the byte `at`: `line L, column C`, counted in characters, or
/// `end of document` at the end of the text.
fn coordinates(text: &str, at: usize) -> String {
    if at >= text.len() {
        return "end of document".to_owned();
    }
    let before = text.get(..at).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = match line {
        1 => before.chars().count() + 1,
        _ => {
            before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count()
                + 1
        }
    };
    format!("line {line}, column {column}")
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, parse_document};

    /// The tomllib detail the parser reports for `text`.
    fn detail(text: &str) -> String {
        match parse_document(text) {
            Err(ConfigError::InvalidToml(detail)) => detail,
            other => panic!("expected invalid TOML, got {other:?}"),
        }
    }

    // The expected texts are tomllib's own (Python 3.12), recorded for these documents.

    #[test]
    fn unclosed_array_of_a_bare_word_is_an_invalid_value_at_the_word() {
        assert_eq!(
            detail("not = [valid\n"),
            "Invalid value (at line 1, column 8)"
        );
    }

    #[test]
    fn missing_value_is_an_invalid_value_at_the_line_end() {
        assert_eq!(detail("a = \n"), "Invalid value (at line 1, column 5)");
    }

    #[test]
    fn unclosed_table_header_is_reported_at_the_line_end() {
        assert_eq!(
            detail("[server\nport = 1\n"),
            "Expected ']' at the end of a table declaration (at line 1, column 8)"
        );
    }

    #[test]
    fn unclosed_array_table_header_is_reported_on_its_line() {
        assert_eq!(
            detail("a = 1\n[[t]\n"),
            "Expected ']]' at the end of an array declaration (at line 2, column 4)"
        );
    }

    #[test]
    fn key_without_assignment_expects_equals() {
        assert_eq!(
            detail("key\n"),
            "Expected '=' after a key in a key/value pair (at line 1, column 4)"
        );
    }

    #[test]
    fn trailing_content_after_a_statement_expects_newline() {
        assert_eq!(
            detail("a = 1 b = 2\n"),
            "Expected newline or end of document after a statement (at line 1, column 7)"
        );
    }

    #[test]
    fn element_after_a_missing_comma_is_an_unclosed_array() {
        assert_eq!(
            detail("a = [1, 2 3]\n"),
            "Unclosed array (at line 1, column 11)"
        );
    }

    #[test]
    fn unclosed_array_across_lines_is_counted_from_the_last_line() {
        assert_eq!(
            detail("a = [\n1,\n2 3]\n"),
            "Unclosed array (at line 3, column 3)"
        );
    }

    #[test]
    fn unclosed_inline_table_is_reported_at_the_line_end() {
        assert_eq!(
            detail("a = {x = 1\n"),
            "Unclosed inline table (at line 1, column 11)"
        );
    }

    #[test]
    fn statement_starting_with_an_equals_sign_is_an_invalid_statement() {
        assert_eq!(detail("= 1\n"), "Invalid statement (at line 1, column 1)");
    }

    #[test]
    fn invalid_value_on_a_later_line_is_counted_from_that_line() {
        assert_eq!(
            detail("port = 0\nx = @\n"),
            "Invalid value (at line 2, column 5)"
        );
    }

    #[test]
    fn failure_at_the_end_of_the_document_has_no_coordinates() {
        assert_eq!(detail("a = "), "Invalid value (at end of document)");
    }

    #[test]
    fn valid_document_parses() {
        let parsed = parse_document("[server]\nport = 1\n");
        assert!(parsed.is_ok(), "{parsed:?}");
    }
}
