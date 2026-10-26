//! Python's `str()` and `repr()` of JSON values. The narrative renders highlights and
//! opportunities with these, so the output matches what the Python code produced.

use serde_json::{Number, Value};

/// `str(value)`: strings as they are, everything else as `repr`.
pub(crate) fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)` of the JSON value as Python's `json.loads` would have built it.
pub(crate) fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number_repr(number),
        Value::String(text) => string_repr(text),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(py_repr)
                .collect::<Vec<String>>()
                .join(", ")
        ),
        Value::Object(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(key, item)| format!("{}: {}", string_repr(key), py_repr(item)))
                .collect::<Vec<String>>()
                .join(", ")
        ),
    }
}

fn number_repr(number: &Number) -> String {
    match number.as_f64() {
        Some(float) if number.is_f64() => float_repr(float),
        _ => number.to_string(),
    }
}

/// `repr(float)`: the shortest round-tripping digits, in positional form for decimal exponents
/// from -4 to 15 and in `d.ddde+XX` form otherwise.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent_text) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent_text.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let body = if (-4..16).contains(&exponent) {
        if exponent >= 0 {
            let integer_len = exponent as usize + 1;
            let padded = format!("{digits:0<width$}", width = integer_len.max(digits.len()));
            let (integer, fraction) = padded.split_at(integer_len);
            let fraction = if fraction.is_empty() { "0" } else { fraction };
            format!("{integer}.{fraction}")
        } else {
            format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{exponent_sign}{:02}", exponent.abs())
    };
    format!("{sign}{body}")
}

/// `repr(str)`: single quotes unless the text holds a single quote and no double quote.
fn string_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c == quote => format!("\\{c}"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("\\x{:02x}", c as u32),
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_follow_python_repr() {
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(float_repr(100.0), "100.0");
        assert_eq!(float_repr(0.0), "0.0");
        assert_eq!(float_repr(0.001), "0.001");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1.5e-5), "1.5e-05");
    }

    #[test]
    fn containers_follow_python_repr() {
        let value = serde_json::json!({"k": "v", "n": [true, null, 1]});
        assert_eq!(py_repr(&value), "{'k': 'v', 'n': [True, None, 1]}");
        assert_eq!(py_str(&Value::String("it's".to_string())), "it's");
        assert_eq!(py_repr(&Value::String("it's".to_string())), "\"it's\"");
    }
}
