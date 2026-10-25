//! Python's text forms: `isoformat`, `strftime`-style `when`, `repr` and `str`.
//!
//! UTC datetimes are formatted by `wg_types::Timestamp`. The functions here cover only the
//! [`LocalDateTime`] values (naive or non-UTC) and the bare dates that `Timestamp` cannot hold.

use wg_types::Timestamp;

use crate::value::{Date, LocalDateTime, PyStr, Value};

/// `datetime.isoformat()` of a naive or non-UTC value: `T` separator, microseconds only when
/// nonzero, `+HH:MM` offset.
pub fn isoformat(dt: &LocalDateTime) -> String {
    format_local(dt, 'T')
}

/// The `when()` helper of the CLI for a naive or non-UTC value: `%Y-%m-%d %H:%M` on the
/// wall-clock fields, `""` for `None`. UTC values use `Timestamp::format_display`.
pub fn when(dt: Option<&LocalDateTime>) -> String {
    match dt {
        None => String::new(),
        Some(d) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            d.year, d.month, d.day, d.hour, d.minute
        ),
    }
}

/// `str(datetime)` of a naive or non-UTC value, which is `isoformat(sep=" ")`.
pub(crate) fn local_datetime_str(dt: &LocalDateTime) -> String {
    format_local(dt, ' ')
}

/// The fields of a UTC timestamp viewed as a zero-offset [`LocalDateTime`], for `repr` only.
pub(crate) fn utc_fields(ts: &Timestamp) -> LocalDateTime {
    let at = ts.to_offset_datetime();
    LocalDateTime {
        year: at.year(),
        month: u8::from(at.month()),
        day: at.day(),
        hour: at.hour(),
        minute: at.minute(),
        second: at.second(),
        microsecond: at.nanosecond() / 1_000,
        offset_seconds: Some(0),
    }
}

/// `str(datetime)` of a UTC timestamp: the `Timestamp` text with a space separator.
pub(crate) fn utc_datetime_str(ts: &Timestamp) -> String {
    ts.format_isoformat().replacen('T', " ", 1)
}

pub(crate) fn date_isoformat(d: &Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

fn format_local(dt: &LocalDateTime, sep: char) -> String {
    let mut text = format!(
        "{:04}-{:02}-{:02}{sep}{:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    );
    if dt.microsecond != 0 {
        text.push_str(&format!(".{:06}", dt.microsecond));
    }
    if let Some(offset) = dt.offset_seconds {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.abs();
        text.push_str(&format!(
            "{sign}{:02}:{:02}",
            magnitude / 3600,
            (magnitude % 3600) / 60
        ));
        if magnitude % 60 != 0 {
            text.push_str(&format!(":{:02}", magnitude % 60));
        }
    }
    text
}

/// Python `float.__repr__`: shortest round-trip digits, fixed notation for decimal exponents
/// from -4 to 15, otherwise scientific with a signed, two-digit-minimum exponent.
pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    // Rust's `{:e}` prints the shortest round-trip digits, e.g. "-1.5e300" or "5e-324".
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exponent) {
        if exponent < 0 {
            let zeros = "0".repeat((-exponent - 1) as usize);
            format!("{sign}0.{zeros}{digits}")
        } else {
            let int_len = exponent as usize + 1;
            if digits.len() <= int_len {
                let zeros = "0".repeat(int_len - digits.len());
                format!("{sign}{digits}{zeros}.0")
            } else {
                format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
            }
        }
    } else {
        let (head, tail) = digits.split_at(1);
        let tail = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{head}{tail}e{exp_sign}{:02}", exponent.abs())
    }
}

/// Python `repr(str)`.
pub(crate) fn py_str_repr(text: &PyStr) -> String {
    let code_points = text.code_points();
    let has_single = code_points.contains(&u32::from('\''));
    let has_double = code_points.contains(&u32::from('"'));
    let quote = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for &cp in code_points {
        if cp == u32::from(quote) || cp == u32::from('\\') {
            out.push('\\');
            out.push(char::from_u32(cp).unwrap_or('?'));
        } else if cp == 0x09 {
            out.push_str("\\t");
        } else if cp == 0x0a {
            out.push_str("\\n");
        } else if cp == 0x0d {
            out.push_str("\\r");
        } else if is_printable(cp) {
            out.push(char::from_u32(cp).unwrap_or('?'));
        } else if cp <= 0xff {
            out.push_str(&format!("\\x{cp:02x}"));
        } else if cp <= 0xffff {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            out.push_str(&format!("\\U{cp:08x}"));
        }
    }
    out.push(quote);
    out
}

/// Approximation of `str.isprintable()` for the code points Python escapes in practice: C0 and C1
/// controls, the space separators, line and paragraph separators, format characters, surrogates,
/// private use and the two noncharacters per plane. Other unassigned code points are not classified.
fn is_printable(cp: u32) -> bool {
    !matches!(
        cp,
        0x00..=0x1f
            | 0x7f..=0xa0
            | 0xad
            | 0x1680
            | 0x2000..=0x200f
            | 0x2028..=0x202f
            | 0x205f..=0x206f
            | 0x3000
            | 0xd800..=0xdfff
            | 0xe000..=0xf8ff
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0xfffe..=0xffff
            | 0xf0000..=0x10ffff
    )
}

/// Python `repr(bytes)`.
pub(crate) fn py_bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if b == quote as u8 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(b)),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(datetime.datetime)`.
fn py_datetime_repr(dt: &LocalDateTime) -> String {
    let mut fields = vec![
        dt.year.to_string(),
        dt.month.to_string(),
        dt.day.to_string(),
        dt.hour.to_string(),
        dt.minute.to_string(),
    ];
    if dt.second != 0 || dt.microsecond != 0 {
        fields.push(dt.second.to_string());
    }
    if dt.microsecond != 0 {
        fields.push(dt.microsecond.to_string());
    }
    let mut text = format!("datetime.datetime({}", fields.join(", "));
    if let Some(offset) = dt.offset_seconds {
        text.push_str(", tzinfo=");
        text.push_str(&py_timezone_repr(offset));
    }
    text.push(')');
    text
}

fn py_timezone_repr(offset: i32) -> String {
    if offset == 0 {
        return "datetime.timezone.utc".to_owned();
    }
    format!("datetime.timezone({})", py_timedelta_repr(offset))
}

fn py_timedelta_repr(seconds: i32) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let mut parts = Vec::new();
    if days != 0 {
        parts.push(format!("days={days}"));
    }
    if rest != 0 {
        parts.push(format!("seconds={rest}"));
    }
    if parts.is_empty() {
        return "datetime.timedelta(0)".to_owned();
    }
    format!("datetime.timedelta({})", parts.join(", "))
}

/// Python `repr(value)` for the values this crate holds.
pub(crate) fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => py_float_repr(*f),
        Value::Str(s) => py_str_repr(s),
        Value::List(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Dict(pairs) => {
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::DateTime(ts) => py_datetime_repr(&utc_fields(ts)),
        Value::LocalDateTime(dt) => py_datetime_repr(dt),
        Value::Date(d) => format!("datetime.date({}, {}, {})", d.year, d.month, d.day),
        Value::Decimal(text) => format!("Decimal('{text}')"),
        Value::Uuid(text) => format!("UUID('{text}')"),
        Value::Bytes(bytes) => py_bytes_repr(bytes),
        Value::Model { name, fields } => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}={}", py_repr(v)))
                .collect();
            format!("{name}({})", inner.join(", "))
        }
    }
}

/// Pydantic's `str(model)`: `field=repr(value)` pairs separated by single spaces, no class name.
pub(crate) fn model_str(fields: &[(String, Value)]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", py_repr(v)))
        .collect();
    parts.join(" ")
}
