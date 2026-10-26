//! rich's JSON highlighting (`rich.highlighter.JSONHighlighter` with the default theme).
//!
//! rich dumps the JSON with `json.dumps(indent=2)` and then styles each token by regular
//! expression: braces, `true`/`false`/`null`, numbers and strings, with a string that is followed
//! by a colon also styled as a key. This module scans the same text the same way and returns the
//! styled runs in order, covering every byte of the input.

use crate::sgr::Span;

/// `json.brace`
const BRACE: &str = "bold";
/// `json.bool_true`
const BOOL_TRUE: &str = "italic bright_green";
/// `json.bool_false`
const BOOL_FALSE: &str = "italic bright_red";
/// `json.null`
const NULL: &str = "italic magenta";
/// `json.number`
const NUMBER: &str = "bold cyan";
/// `json.str`
const STRING: &str = "green";
/// `json.key`, which rich applies over `json.str`; the combined style is `bold blue`.
const KEY: &str = "bold blue";

/// Splits `text` into styled runs. Structural characters, whitespace and anything unrecognised
/// come back as unstyled runs, so joining the texts gives `text` back exactly.
#[must_use]
pub fn highlight_json(text: &str) -> Vec<Span<'_>> {
    let bytes = text.as_bytes();
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while i < bytes.len() {
        let (end, style) = match bytes[i] {
            b'{' | b'}' | b'[' | b']' => (i + 1, BRACE),
            b'"' => {
                let end = string_end(bytes, i);
                let style = if is_key(bytes, end) { KEY } else { STRING };
                (end, style)
            }
            b'-' | b'0'..=b'9' if number_end(bytes, i) > i => (number_end(bytes, i), NUMBER),
            b't' if bytes[i..].starts_with(b"true") => (i + 4, BOOL_TRUE),
            b'f' if bytes[i..].starts_with(b"false") => (i + 5, BOOL_FALSE),
            b'n' if bytes[i..].starts_with(b"null") => (i + 4, NULL),
            _ => {
                i += 1;
                continue;
            }
        };
        if plain_from < i {
            spans.push(run(text, plain_from, i, ""));
        }
        spans.push(run(text, i, end, style));
        plain_from = end;
        i = end;
    }
    if plain_from < text.len() {
        spans.push(run(text, plain_from, text.len(), ""));
    }
    spans
}

fn run<'a>(text: &'a str, from: usize, to: usize, style: &'static str) -> Span<'a> {
    Span {
        text: &text[from..to],
        style,
    }
}

/// The index just past the closing quote of the string starting at `start`. An escaped quote
/// does not close it; an unterminated string runs to the end of the text.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Whether a colon follows the string ending at `end`, after optional whitespace (rich's
/// `JSON_WHITESPACE`: space, newline, carriage return, tab).
fn is_key(bytes: &[u8], end: usize) -> bool {
    bytes[end.min(bytes.len())..]
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        .is_some_and(|byte| *byte == b':')
}

/// The index just past the number starting at `start`: `-?digits(.digits)?(e[+-]?digits)?`.
/// Returns `start` when there is no number there (a minus sign with no digit after it).
fn number_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if bytes.get(i) == Some(&b'-') {
        i += 1;
    }
    let int_end = digits_end(bytes, i);
    if int_end == i {
        return start;
    }
    i = int_end;
    if bytes.get(i) == Some(&b'.') {
        i = digits_end(bytes, i + 1);
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut exponent = i + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let after = digits_end(bytes, exponent);
        if after > exponent {
            i = after;
        }
    }
    i
}

fn digits_end(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_cover_the_text_exactly() {
        let text = "{\n  \"a\": [1, -2.5e3, true],\n  \"b\": \"x\\\"y\"\n}\n";
        let spans = highlight_json(text);
        let joined: String = spans.iter().map(|s| s.text).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn styles_follow_rich_theme() {
        let spans = highlight_json("{\"k\": [null, false]}");
        let styled: Vec<(&str, &str)> = spans
            .iter()
            .filter(|s| !s.style.is_empty())
            .map(|s| (s.text, s.style))
            .collect();
        assert_eq!(
            styled,
            vec![
                ("{", "bold"),
                ("\"k\"", "bold blue"),
                ("[", "bold"),
                ("null", "italic magenta"),
                ("false", "italic bright_red"),
                ("]", "bold"),
                ("}", "bold"),
            ]
        );
    }

    #[test]
    fn string_values_are_not_keys_and_escapes_do_not_end_strings() {
        let spans = highlight_json("[\"a\\\\\", \"true\"]");
        let styled: Vec<&str> = spans
            .iter()
            .filter(|s| s.style == "green")
            .map(|s| s.text)
            .collect();
        assert_eq!(styled, vec!["\"a\\\\\"", "\"true\""]);
    }
}
