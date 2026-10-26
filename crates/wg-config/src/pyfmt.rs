//! Python's text and number conversions, reproduced where the configuration output depends on them:
//! `repr()` of strings, floats and values, and the bound text of Pydantic messages.

use serde_json::Value;

/// `repr()` of a `str`: single quotes unless the text holds a single quote and no double quote.
pub(crate) fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
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

/// `repr()` of a `float`, as `tomli_w` writes it: the shortest round-trip digits, in fixed notation
/// for decimal exponents from -4 to 15 and in `e+NN` / `e-NN` notation otherwise.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = match scientific.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap_or(0)),
        None => (scientific.as_str(), 0),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-4..16).contains(&exponent) {
        let body = if exponent >= 0 {
            let split = usize::try_from(exponent).unwrap_or(0) + 1;
            let (whole, frac) = if digits.len() > split {
                (&digits[..split], &digits[split..])
            } else {
                (digits.as_str(), "")
            };
            let zeros = "0".repeat(split.saturating_sub(digits.len()));
            format!(
                "{whole}{zeros}.{}",
                if frac.is_empty() { "0" } else { frac }
            )
        } else {
            let zeros = "0".repeat(usize::try_from(-exponent - 1).unwrap_or(0));
            format!("0.{zeros}{digits}")
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exp_sign = if exponent < 0 { "-" } else { "+" };
        format!("{sign}{first}{fraction}e{exp_sign}{:02}", exponent.abs())
    }
}

/// Python's `repr()` of a configuration value read from TOML: `None`, `True`/`False`, numbers,
/// quoted strings, and lists and dicts of those.
pub fn value_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => match number.as_i64() {
            Some(int) => int.to_string(),
            None => float_repr(number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(text) => repr_str(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(value_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{}: {}", repr_str(key), value_repr(value)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// A bound as Pydantic prints it in a message: an integer-valued float prints without a fraction.
pub(crate) fn bound_text(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.abs() < 9.0e15 {
        format!("{}", bound as i64)
    } else {
        float_repr(bound)
    }
}

/// Pydantic's `one of` list for a literal: `'a', 'b' or 'c'`.
pub(crate) fn one_of<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = values.map(|value| format!("'{value}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}
