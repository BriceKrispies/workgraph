//! GV-12: every case of `golden/jsonfmt/cases.json` must produce the recorded output or the
//! recorded Python exception name, in each dialect the case lists.

use serde_json::Value as Json;
use wg_json::{Date, DateTime, PyStr, Value, cli_json, db_json, http_json, isoformat, when};

const CASES_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../golden/jsonfmt/cases.json"
);

/// `serde_json` rejects lone surrogate escapes, which the golden file uses for one case. Each lone
/// surrogate escape is replaced by a private-use marker before parsing and mapped back afterwards.
const MARK_BASE: u32 = 0xF0000;

fn preprocess(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // An escape. Other than \u, copy the backslash and the escaped character together, so an
        // escaped backslash is never mistaken for the start of a \u escape.
        if chars.get(i + 1) != Some(&'u') {
            out.extend(chars[i..(i + 2).min(chars.len())].iter());
            i += 2;
            continue;
        }
        let Some(unit) = hex4(&chars, i + 2) else {
            out.extend(chars[i..(i + 2).min(chars.len())].iter());
            i += 2;
            continue;
        };
        if (0xd800..=0xdbff).contains(&unit)
            && chars.get(i + 6) == Some(&'\\')
            && chars.get(i + 7) == Some(&'u')
            && hex4(&chars, i + 8).is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
        {
            out.extend(chars[i..i + 12].iter());
            i += 12;
        } else if (0xd800..=0xdfff).contains(&unit) {
            out.push(char::from_u32(MARK_BASE + (unit - 0xd800)).unwrap_or('?'));
            i += 6;
        } else {
            out.extend(chars[i..i + 6].iter());
            i += 6;
        }
    }
    out
}

fn hex4(chars: &[char], at: usize) -> Option<u32> {
    let digits: String = chars.get(at..at + 4)?.iter().collect();
    u32::from_str_radix(&digits, 16).ok()
}

/// Converts a golden string to code points, turning the markers back into lone surrogates.
fn pystr(text: &str) -> PyStr {
    let code_points = text
        .chars()
        .map(|c| {
            let cp = u32::from(c);
            if (MARK_BASE..MARK_BASE + 0x800).contains(&cp) {
                0xd800 + (cp - MARK_BASE)
            } else {
                cp
            }
        })
        .collect();
    PyStr::from_code_points(code_points)
}

fn str_of<'a>(node: &'a Json, what: &str) -> Result<&'a str, String> {
    node.as_str()
        .ok_or_else(|| format!("{what} is not a string"))
}

fn parse_offset(text: &str) -> Result<i32, String> {
    if text == "Z" {
        return Ok(0);
    }
    let sign = match text.chars().next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return Err(format!("bad offset {text}")),
    };
    let (hours, minutes) = text[1..]
        .split_once(':')
        .ok_or_else(|| format!("bad offset {text}"))?;
    let hours: i32 = hours.parse().map_err(|_| format!("bad offset {text}"))?;
    let minutes: i32 = minutes.parse().map_err(|_| format!("bad offset {text}"))?;
    Ok(sign * (hours * 3600 + minutes * 60))
}

/// Parses `YYYY-MM-DD[THH:MM:SS[.ffffff]][offset]` as `datetime.fromisoformat` does for the
/// forms the vectors use.
fn parse_datetime(text: &str) -> Result<DateTime, String> {
    let (date_part, time_part) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (text, None),
    };
    let date = parse_date(date_part)?;
    let (clock, offset) = match time_part {
        None => (None, None),
        Some(time) => {
            let split = time.find(['+', '-', 'Z']);
            match split {
                Some(idx) => (Some(&time[..idx]), Some(parse_offset(&time[idx..])?)),
                None => (Some(time), None),
            }
        }
    };
    let (hour, minute, second, microsecond) = match clock {
        None => (0, 0, 0, 0),
        Some(clock) => {
            let (hms, fraction) = match clock.split_once('.') {
                Some((hms, frac)) => (hms, frac),
                None => (clock, ""),
            };
            let mut fields = hms.split(':');
            let mut next = || -> Result<u8, String> {
                fields
                    .next()
                    .ok_or_else(|| format!("bad time {clock}"))?
                    .parse()
                    .map_err(|_| format!("bad time {clock}"))
            };
            let hour = next()?;
            let minute = next()?;
            let second = next()?;
            let micro = if fraction.is_empty() {
                0
            } else {
                format!("{fraction:0<6}")[..6]
                    .parse::<u32>()
                    .map_err(|_| format!("bad fraction {fraction}"))?
            };
            (hour, minute, second, micro)
        }
    };
    Ok(DateTime {
        year: date.year,
        month: date.month,
        day: date.day,
        hour,
        minute,
        second,
        microsecond,
        offset_seconds: offset,
    })
}

fn parse_date(text: &str) -> Result<Date, String> {
    let mut fields = text.split('-');
    let year: i32 = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad date {text}"))?;
    let month: u8 = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad date {text}"))?;
    let day: u8 = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad date {text}"))?;
    Ok(Date { year, month, day })
}

fn build(node: &Json) -> Result<Value, String> {
    let kind = str_of(&node["t"], "t")?;
    Ok(match kind {
        "null" => Value::Null,
        "bool" => Value::Bool(node["v"].as_bool().ok_or("bad bool")?),
        "int" => Value::Int(
            node["v"]
                .to_string()
                .parse()
                .map_err(|_| "bad int".to_owned())?,
        ),
        "float" => Value::Float(
            str_of(&node["v"], "float")?
                .parse()
                .map_err(|_| "bad float".to_owned())?,
        ),
        "str" => Value::Str(pystr(str_of(&node["v"], "str")?)),
        "datetime" => Value::DateTime(parse_datetime(str_of(&node["v"], "datetime")?)?),
        "date" => Value::Date(parse_date(str_of(&node["v"], "date")?)?),
        "decimal" => Value::Decimal(str_of(&node["v"], "decimal")?.to_owned()),
        "uuid" => Value::Uuid(str_of(&node["v"], "uuid")?.to_owned()),
        "bytes" => Value::Bytes(parse_hex(str_of(&node["v"], "bytes")?)?),
        "list" | "tuple" => Value::List(items(&node["v"])?),
        "dict" => Value::Dict(
            pairs(&node["v"])?
                .into_iter()
                .map(|(k, v)| Ok((pystr(&k), v)))
                .collect::<Result<_, String>>()?,
        ),
        "model" => Value::Model {
            name: str_of(&node["name"], "name")?.to_owned(),
            fields: pairs(&node["v"])?,
        },
        other => return Err(format!("unknown node type {other}")),
    })
}

fn items(node: &Json) -> Result<Vec<Value>, String> {
    node.as_array()
        .ok_or("bad list")?
        .iter()
        .map(build)
        .collect()
}

fn pairs(node: &Json) -> Result<Vec<(String, Value)>, String> {
    node.as_array()
        .ok_or("bad pairs")?
        .iter()
        .map(|pair| {
            let key = str_of(&pair[0], "key")?.to_owned();
            Ok((key, build(&pair[1])?))
        })
        .collect()
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| format!("bad hex {text}")))
        .collect()
}

/// Runs one dialect. `Ok(None)` is SQL NULL; `Err` is the Python exception class name.
fn run(dialect: &str, value: &Value) -> Result<Option<String>, String> {
    let named = |e: wg_json::DumpError| e.python_name().to_owned();
    match dialect {
        "O01" => cli_json(value).map(Some).map_err(named),
        "O06" => http_json(value).map(Some).map_err(named),
        "O10" => db_json(value).map_err(named),
        "isoformat" => match value {
            Value::DateTime(dt) => Ok(Some(isoformat(dt))),
            _ => Err("TypeError".to_owned()),
        },
        "when" => match value {
            Value::DateTime(dt) => Ok(Some(when(Some(dt)))),
            Value::Null => Ok(Some(when(None))),
            _ => Err("TypeError".to_owned()),
        },
        other => Err(format!("unknown dialect {other}")),
    }
}

#[test]
fn gv12_jsonfmt_vectors_pass() -> Result<(), String> {
    let raw = std::fs::read_to_string(CASES_PATH).map_err(|e| e.to_string())?;
    let doc: Json = serde_json::from_str(&preprocess(&raw)).map_err(|e| e.to_string())?;
    let cases = doc["cases"].as_array().ok_or("no cases array")?;
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for case in cases {
        let id = str_of(&case["id"], "id")?;
        let value = build(&case["input"]).map_err(|e| format!("{id}: {e}"))?;
        let outputs = case["outputs"]
            .as_object()
            .ok_or_else(|| format!("{id}: no outputs"))?;
        for (dialect, expected) in outputs {
            checked += 1;
            let got = run(dialect, &value);
            let matches = if let Some(error) = expected.get("error") {
                got == Err(error.as_str().unwrap_or_default().to_owned())
            } else {
                match expected.get("output") {
                    Some(Json::Null) => got == Ok(None),
                    Some(Json::String(text)) => {
                        got.as_ref().ok().and_then(|o| o.as_deref()) == Some(text.as_str())
                    }
                    _ => false,
                }
            };
            if !matches {
                failures.push(format!(
                    "{id} [{dialect}]: expected {expected}, got {got:?}"
                ));
            }
        }
    }
    assert!(checked > 0, "no golden outputs were checked");
    assert!(
        failures.is_empty(),
        "{} of {checked} golden outputs differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}
