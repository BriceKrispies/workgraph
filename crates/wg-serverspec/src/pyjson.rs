//! `json.loads(body)` as Python 3.12 runs it on a request body: the value, or the
//! `JSONDecodeError` message and character position that FastAPI turns into a `json_invalid` 422
//! item, or a body that is not text at all (FastAPI's 400).
//!
//! The scanner follows CPython's `_json.c` step for step, so each malformed body fails with the
//! same message at the same position. The bytes are decoded as `json.loads` does: the encoding
//! `json.detect_encoding` picks (UTF-8, UTF-16 or UTF-32, with or without a byte-order mark).
//! `NaN` and `Infinity` are floats to Python, so they are written as the validator's non-finite
//! marker (`wg_validate::non_finite_marker`); an integer beyond 64 bits that overflows is `null`.

use serde_json::{Map, Number, Value};
use wg_validate::non_finite_marker;

/// What decoding a body gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    Value(Value),
    /// `JSONDecodeError(msg, doc, pos)`.
    Invalid {
        message: &'static str,
        position: usize,
    },
    /// The bytes are not UTF-8 text.
    NotText,
}

struct Failure {
    message: &'static str,
    position: usize,
}

fn fail<T>(message: &'static str, position: usize) -> Result<T, Failure> {
    Err(Failure { message, position })
}

const EXPECTING_VALUE: &str = "Expecting value";

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

/// `json.loads(bytes)`: the bytes decoded as `json.detect_encoding` picks, then parsed.
pub fn loads(body: &[u8]) -> Decoded {
    let Some(text) = decode(body) else {
        return Decoded::NotText;
    };
    let chars: Vec<char> = text.chars().collect();
    let scanner = Scanner { s: &chars };
    let start = scanner.ws(0);
    let result = scanner.value(start).and_then(|(value, end)| {
        let end = scanner.ws(end);
        if end == chars.len() {
            Ok(value)
        } else {
            fail("Extra data", end)
        }
    });
    match result {
        Ok(value) => Decoded::Value(value),
        Err(Failure { message, position }) => Decoded::Invalid { message, position },
    }
}

/// `s.decode(json.detect_encoding(s), 'surrogatepass')`: the text of the body, or `None` when the
/// bytes are not text in the encoding detected (FastAPI's 400). The detection follows
/// `json.detect_encoding`: a byte-order mark first, then the zero bytes of the first characters.
fn decode(body: &[u8]) -> Option<String> {
    if let Some(rest) = body.strip_prefix(&[0xff, 0xfe, 0, 0]) {
        return utf32(rest, Endian::Little);
    }
    if let Some(rest) = body.strip_prefix(&[0, 0, 0xfe, 0xff]) {
        return utf32(rest, Endian::Big);
    }
    if let Some(rest) = body.strip_prefix(&[0xff, 0xfe]) {
        return utf16(rest, Endian::Little);
    }
    if let Some(rest) = body.strip_prefix(&[0xfe, 0xff]) {
        return utf16(rest, Endian::Big);
    }
    if let Some(rest) = body.strip_prefix(b"\xef\xbb\xbf") {
        return utf8(rest);
    }
    if body.len() >= 4 {
        if body[0] == 0 {
            return if body[1] != 0 {
                utf16(body, Endian::Big)
            } else {
                utf32(body, Endian::Big)
            };
        }
        if body[1] == 0 {
            return if body[2] != 0 || body[3] != 0 {
                utf16(body, Endian::Little)
            } else {
                utf32(body, Endian::Little)
            };
        }
    } else if body.len() == 2 {
        if body[0] == 0 {
            return utf16(body, Endian::Big);
        }
        if body[1] == 0 {
            return utf16(body, Endian::Little);
        }
    }
    utf8(body)
}

fn utf8(body: &[u8]) -> Option<String> {
    std::str::from_utf8(body).ok().map(str::to_owned)
}

/// UTF-16 code units; an odd byte count or a bad unit is not text. A lone surrogate, which Python
/// keeps, becomes U+FFFD because a Rust string cannot hold it.
fn utf16(body: &[u8], endian: Endian) -> Option<String> {
    if !body.len().is_multiple_of(2) {
        return None;
    }
    let units = body.chunks_exact(2).map(|pair| {
        let bytes = [pair[0], pair[1]];
        match endian {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        }
    });
    Some(
        char::decode_utf16(units)
            .map(|unit| unit.unwrap_or('\u{fffd}'))
            .collect(),
    )
}

/// UTF-32 code points; a length that is not a multiple of four, or a value past U+10FFFF, is not
/// text. A surrogate code point becomes U+FFFD, as for UTF-16.
fn utf32(body: &[u8], endian: Endian) -> Option<String> {
    if !body.len().is_multiple_of(4) {
        return None;
    }
    let mut text = String::with_capacity(body.len() / 4);
    for quad in body.chunks_exact(4) {
        let bytes = [quad[0], quad[1], quad[2], quad[3]];
        let code = match endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        };
        if code > 0x10ffff {
            return None;
        }
        text.push(char::from_u32(code).unwrap_or('\u{fffd}'));
    }
    Some(text)
}

struct Scanner<'a> {
    s: &'a [char],
}

impl Scanner<'_> {
    fn at(&self, idx: usize) -> Option<char> {
        self.s.get(idx).copied()
    }

    fn ws(&self, idx: usize) -> usize {
        let mut idx = idx;
        while matches!(self.at(idx), Some(' ' | '\t' | '\n' | '\r')) {
            idx += 1;
        }
        idx
    }

    fn literal(&self, idx: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(offset, ch)| self.at(idx + offset) == Some(ch))
    }

    /// `scan_once(s, idx)`; a `StopIteration` is "Expecting value" at its index.
    fn value(&self, idx: usize) -> Result<(Value, usize), Failure> {
        match self.at(idx) {
            None => fail(EXPECTING_VALUE, idx),
            Some('"') => self
                .string(idx + 1)
                .map(|(text, end)| (Value::String(text), end)),
            Some('{') => self.object(idx + 1),
            Some('[') => self.array(idx + 1),
            Some('n') if self.literal(idx, "null") => Ok((Value::Null, idx + 4)),
            Some('t') if self.literal(idx, "true") => Ok((Value::Bool(true), idx + 4)),
            Some('f') if self.literal(idx, "false") => Ok((Value::Bool(false), idx + 5)),
            Some('N') if self.literal(idx, "NaN") => Ok((non_finite_marker(f64::NAN), idx + 3)),
            Some('I') if self.literal(idx, "Infinity") => {
                Ok((non_finite_marker(f64::INFINITY), idx + 8))
            }
            Some('-') if self.literal(idx, "-Infinity") => {
                Ok((non_finite_marker(f64::NEG_INFINITY), idx + 9))
            }
            Some(_) => self.number(idx),
        }
    }

    fn digits(&self, idx: usize) -> usize {
        let mut idx = idx;
        while self.at(idx).is_some_and(|c| c.is_ascii_digit()) {
            idx += 1;
        }
        idx
    }

    /// `_match_number_unicode`.
    fn number(&self, start: usize) -> Result<(Value, usize), Failure> {
        let mut idx = start;
        if self.at(idx) == Some('-') {
            idx += 1;
        }
        match self.at(idx) {
            Some('1'..='9') => idx = self.digits(idx + 1),
            Some('0') => idx += 1,
            _ => return fail(EXPECTING_VALUE, start),
        }
        let integer_end = idx;
        let mut is_float = false;
        if self.at(idx) == Some('.') && self.at(idx + 1).is_some_and(|c| c.is_ascii_digit()) {
            idx = self.digits(idx + 1);
            is_float = true;
        }
        if matches!(self.at(idx), Some('e' | 'E')) {
            let mut exp = idx + 1;
            if matches!(self.at(exp), Some('+' | '-')) {
                exp += 1;
            }
            let end = self.digits(exp);
            if end > exp {
                idx = end;
                is_float = true;
            }
        }
        let text: String = self.s[start..idx].iter().collect();
        let value = if is_float || integer_end != idx {
            float_value(&text)
        } else {
            text.parse::<i64>()
                .map(Number::from)
                .or_else(|_| text.parse::<u64>().map(Number::from))
                .map(Value::Number)
                .unwrap_or_else(|_| {
                    text.parse::<f64>()
                        .ok()
                        .and_then(Number::from_f64)
                        .map_or(Value::Null, Value::Number)
                })
        };
        Ok((value, idx))
    }

    /// `scanstring(s, end)`: `end` is the index after the opening quote.
    fn string(&self, end: usize) -> Result<(String, usize), Failure> {
        let begin = end - 1;
        let mut out = String::new();
        let mut next = end;
        loop {
            let Some(c) = self.at(next) else {
                return fail("Unterminated string starting at", begin);
            };
            match c {
                '"' => return Ok((out, next + 1)),
                '\\' => {
                    next += 1;
                    let Some(escape) = self.at(next) else {
                        return fail("Unterminated string starting at", begin);
                    };
                    if escape == 'u' {
                        let (ch, after) = self.unicode_escape(next + 1)?;
                        out.push(ch);
                        next = after;
                    } else {
                        let decoded = match escape {
                            '"' => '"',
                            '\\' => '\\',
                            '/' => '/',
                            'b' => '\u{8}',
                            'f' => '\u{c}',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            _ => return fail("Invalid \\escape", next - 1),
                        };
                        out.push(decoded);
                        next += 1;
                    }
                }
                c if (c as u32) <= 0x1f => return fail("Invalid control character at", next),
                c => {
                    out.push(c);
                    next += 1;
                }
            }
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        (0..4).try_fold(0u32, |acc, offset| {
            self.at(at + offset)
                .and_then(|c| c.to_digit(16))
                .map(|digit| acc * 16 + digit)
        })
    }

    /// The four hex digits after `\u` at `next`, joining a surrogate pair. A lone surrogate,
    /// which a Rust string cannot hold, becomes U+FFFD.
    fn unicode_escape(&self, next: usize) -> Result<(char, usize), Failure> {
        let end = next + 4;
        if end >= self.s.len() {
            return fail("Invalid \\uXXXX escape", next - 1);
        }
        let Some(high) = self.hex4(next) else {
            return fail("Invalid \\uXXXX escape", end - 5);
        };
        if (0xd800..0xdc00).contains(&high)
            && end + 6 < self.s.len()
            && self.at(end) == Some('\\')
            && self.at(end + 1) == Some('u')
        {
            let Some(low) = self.hex4(end + 2) else {
                return fail("Invalid \\uXXXX escape", end + 1);
            };
            if (0xdc00..0xe000).contains(&low) {
                let joined = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00);
                return Ok((char::from_u32(joined).unwrap_or('\u{fffd}'), end + 6));
            }
        }
        Ok((char::from_u32(high).unwrap_or('\u{fffd}'), end))
    }

    /// `_parse_object_unicode`: `idx` is the index after `{`.
    fn object(&self, idx: usize) -> Result<(Value, usize), Failure> {
        let mut map = Map::new();
        let mut idx = self.ws(idx);
        if self.at(idx) == Some('}') {
            return Ok((Value::Object(map), idx + 1));
        }
        loop {
            if self.at(idx) != Some('"') {
                return fail("Expecting property name enclosed in double quotes", idx);
            }
            let (key, after) = self.string(idx + 1)?;
            idx = self.ws(after);
            if self.at(idx) != Some(':') {
                return fail("Expecting ':' delimiter", idx);
            }
            idx = self.ws(idx + 1);
            let (value, after) = self.value(idx)?;
            map.insert(key, value);
            idx = self.ws(after);
            match self.at(idx) {
                Some('}') => return Ok((Value::Object(map), idx + 1)),
                Some(',') => idx = self.ws(idx + 1),
                _ => return fail("Expecting ',' delimiter", idx),
            }
        }
    }

    /// `_parse_array_unicode`: `idx` is the index after `[`.
    fn array(&self, idx: usize) -> Result<(Value, usize), Failure> {
        let mut items = Vec::new();
        let mut idx = self.ws(idx);
        if self.at(idx) == Some(']') {
            return Ok((Value::Array(items), idx + 1));
        }
        loop {
            let (value, after) = self.value(idx)?;
            items.push(value);
            idx = self.ws(after);
            match self.at(idx) {
                Some(']') => return Ok((Value::Array(items), idx + 1)),
                Some(',') => idx = self.ws(idx + 1),
                _ => return fail("Expecting ',' delimiter", idx),
            }
        }
    }
}

/// A float literal: `float(text)`, which overflows to an infinity for `1e999` as Python does.
fn float_value(text: &str) -> Value {
    match text.parse::<f64>() {
        Ok(number) if number.is_finite() => {
            Number::from_f64(number).map_or(Value::Null, Value::Number)
        }
        Ok(number) => non_finite_marker(number),
        Err(_) => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invalid(body: &str) -> (&'static str, usize) {
        match loads(body.as_bytes()) {
            Decoded::Invalid { message, position } => (message, position),
            other => panic!("{body:?} decoded: {other:?}"),
        }
    }

    #[test]
    fn decodes_values() {
        assert_eq!(
            loads(br#" {"a": [1, 2.5, -0, true, null, "x\u00e9\ud83d\ude00"], "a": 3} "#),
            Decoded::Value(json!({"a": 3}))
        );
        assert_eq!(loads(b"\xef\xbb\xbf[]"), Decoded::Value(json!([])));
        assert_eq!(loads(b"\xff"), Decoded::NotText);
        assert_eq!(loads(b"1e5"), Decoded::Value(json!(100000.0)));
    }

    #[test]
    fn decodes_utf16_and_utf32_bodies() {
        let text = r#"{"parent": "WG-1"}"#;
        let expected = Decoded::Value(json!({"parent": "WG-1"}));
        for encoding in [
            "utf-8",
            "utf-16",
            "utf-16-le",
            "utf-16-be",
            "utf-32",
            "utf-32-le",
            "utf-32-be",
        ] {
            let bytes = match encoding {
                "utf-8" => text.as_bytes().to_vec(),
                "utf-16" => [&[0xff, 0xfe][..], &utf16le(text)].concat(),
                "utf-16-le" => utf16le(text),
                "utf-16-be" => utf16be(text),
                "utf-32" => [&[0xff, 0xfe, 0, 0][..], &utf32le(text)].concat(),
                "utf-32-le" => utf32le(text),
                _ => utf32be(text),
            };
            assert_eq!(loads(&bytes), expected, "{encoding}");
        }
        assert_eq!(
            loads(&[&[0xef, 0xbb, 0xbf][..], text.as_bytes()].concat()),
            expected
        );
        // A UTF-16 body that is not text: an odd byte count, or a lone byte.
        // Three bytes are UTF-8 to Python (no detection applies), so "[" then a NUL: "Expecting value".
        assert_eq!(
            loads(&utf16le("[]")[..3]),
            Decoded::Invalid {
                message: "Expecting value",
                position: 1,
            }
        );
        assert_eq!(
            loads(b"\xff\xfe{\x00"),
            Decoded::Invalid {
                message: "Expecting property name enclosed in double quotes",
                position: 1,
            }
        );
        assert_eq!(loads(&utf32le("[]")[..6]), Decoded::NotText);
    }

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn utf16be(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    fn utf32le(text: &str) -> Vec<u8> {
        text.chars()
            .flat_map(|c| (c as u32).to_le_bytes())
            .collect()
    }

    fn utf32be(text: &str) -> Vec<u8> {
        text.chars()
            .flat_map(|c| (c as u32).to_be_bytes())
            .collect()
    }

    #[test]
    fn non_finite_floats_are_markers() {
        assert_eq!(loads(b"NaN"), Decoded::Value(non_finite_marker(f64::NAN)));
        assert_eq!(
            loads(b"[Infinity, -Infinity]"),
            Decoded::Value(json!([
                non_finite_marker(f64::INFINITY),
                non_finite_marker(f64::NEG_INFINITY)
            ]))
        );
        assert_eq!(
            loads(b"1e999"),
            Decoded::Value(non_finite_marker(f64::INFINITY))
        );
        assert_eq!(
            loads(b"-1e999"),
            Decoded::Value(non_finite_marker(f64::NEG_INFINITY))
        );
        // Not a non-finite literal: the scanner's own error.
        assert_eq!(invalid("Nan"), ("Expecting value", 0));
    }

    #[test]
    fn errors_like_python() {
        // Each pair is what Python 3.12's json.loads raises (msg, pos), as FastAPI reported them.
        assert_eq!(
            invalid("{"),
            ("Expecting property name enclosed in double quotes", 1)
        );
        assert_eq!(
            invalid("{\"a\":1,}"),
            ("Expecting property name enclosed in double quotes", 7)
        );
        assert_eq!(invalid("[1,]"), ("Expecting value", 3));
        assert_eq!(invalid("{\"a\" 1}"), ("Expecting ':' delimiter", 5));
        assert_eq!(invalid("\"abc"), ("Unterminated string starting at", 0));
        assert_eq!(invalid("1 2"), ("Extra data", 2));
        assert_eq!(invalid("\"a\\qb\""), ("Invalid \\escape", 2));
        assert_eq!(invalid("\"a\nb\""), ("Invalid control character at", 2));
        assert_eq!(invalid(""), ("Expecting value", 0));
        assert_eq!(invalid("  "), ("Expecting value", 2));
        assert_eq!(invalid("-"), ("Expecting value", 0));
        assert_eq!(invalid("[1 2]"), ("Expecting ',' delimiter", 3));
        assert_eq!(invalid("01"), ("Extra data", 1));
        assert_eq!(invalid("\"\\u12\""), ("Invalid \\uXXXX escape", 2));
        assert_eq!(invalid("\"\\u12zz\""), ("Invalid \\uXXXX escape", 2));
        assert_eq!(invalid("\"\\ud800\\u12\""), ("Invalid \\uXXXX escape", 8));
        assert_eq!(
            invalid("\"\\ud800\\u12zzzzzz\""),
            ("Invalid \\uXXXX escape", 8)
        );
        assert_eq!(invalid("1.5e+"), ("Extra data", 3));
        assert_eq!(invalid("["), ("Expecting value", 1));
        assert_eq!(invalid("{\"a\""), ("Expecting ':' delimiter", 4));
        assert_eq!(invalid("{\"a\":1"), ("Expecting ',' delimiter", 6));
        assert_eq!(invalid("\"\\"), ("Unterminated string starting at", 0));
    }
}
