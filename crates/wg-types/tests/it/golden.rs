//! Golden vectors for wg-types: GV-12 timestamp formatting and GV-01 state names.
//!
//! Vectors are produced by `tools/golden/gen_jsonfmt.py` and `tools/golden/gen_lifecycle.py`
//! from the Python code. Cases whose input is not a UTC instant are not representable in
//! `Timestamp` (it is UTC-only); they are counted and reported, not compared.

use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use wg_types::{AttemptStatus, TaskStatus, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

fn golden_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden")
        .join(relative)
}

fn load(relative: &str) -> Result<Value, Box<dyn Error>> {
    let text = std::fs::read_to_string(golden_path(relative))?;
    Ok(serde_json::from_str(&replace_surrogate_escapes(&text))?)
}

/// `golden/jsonfmt/cases.json` holds lone-surrogate escapes (`"\ud800"`) from Python's `json`,
/// which serde_json rejects. Each such escape is replaced by U+FFFD. Only string-escape vectors
/// contain them, and none of the timestamp cases read those strings.
fn replace_surrogate_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let escape = &rest[at..];
        let is_surrogate = escape.len() >= 6
            && escape.as_bytes()[1] == b'u'
            && matches!(
                escape.get(2..6).map(|hex| u16::from_str_radix(hex, 16)),
                Some(Ok(0xD800..=0xDFFF))
            );
        if is_surrogate {
            out.push_str("\\uFFFD");
            rest = &escape[6..];
        } else {
            // Copy the backslash and the escaped character together, so `\\` is never misread.
            let next = escape.chars().nth(1).map_or(1, char::len_utf8);
            out.push_str(&escape[..1 + next]);
            rest = &escape[1 + next..];
        }
    }
    out.push_str(rest);
    out
}

fn str_field<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str, Box<dyn Error>> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| format!("missing key {key}"))?;
    }
    current
        .as_str()
        .ok_or_else(|| format!("{path:?} is not a string").into())
}

/// The UTC instant a golden input denotes, or `None` when the input is naive or has an offset
/// other than UTC.
fn utc_input(text: &str) -> Result<Option<Timestamp>, Box<dyn Error>> {
    let is_utc = text.ends_with('Z') || text.ends_with("+00:00");
    if !is_utc {
        return Ok(None);
    }
    Ok(Some(Timestamp::parse_iso(text)?))
}

/// The database form of a Python `isoformat()` string: fraction padded to six digits.
fn db_form(isoformat: &str) -> String {
    let body = isoformat.strip_suffix("+00:00").unwrap_or(isoformat);
    let padded = match body.split_once('.') {
        Some((head, fraction)) => format!("{head}.{fraction:0<6}"),
        None => format!("{body}.000000"),
    };
    format!("{padded}+00:00")
}

#[test]
fn gv12_timestamp_formatters_match_python() -> TestResult {
    let golden = load("jsonfmt/cases.json")?;
    let cases = golden["cases"].as_array().ok_or("cases is not an array")?;
    let mut compared = 0usize;
    let mut skipped = 0usize;

    for case in cases {
        let group = str_field(case, &["group"])?;
        if group != "datetime" && group != "when" {
            continue;
        }
        let input = &case["input"];
        if input["t"] != "datetime" {
            // `when(None)` and other non-datetime inputs are not timestamps.
            skipped += 1;
            continue;
        }
        let text = str_field(input, &["v"])?;
        let Some(stamp) = utc_input(text)? else {
            skipped += 1;
            continue;
        };

        if group == "when" {
            let expected = str_field(case, &["outputs", "when", "output"])?;
            assert_eq!(stamp.format_display(), expected, "when {text}");
        } else {
            let id = str_field(case, &["id"])?;
            let isoformat = str_field(case, &["outputs", "isoformat", "output"])?;
            assert_eq!(stamp.format_isoformat(), isoformat, "isoformat {id}");
            assert_eq!(stamp.format_db(), db_form(isoformat), "db form {id}");
            // Python's `str(datetime)` (O10 for a nested datetime) is the isoformat with a
            // space separator; it must agree with the isoformat formatter.
            // O10's output is the JSON encoding of that string.
            let o10: String =
                serde_json::from_str(str_field(case, &["outputs", "O10", "output"])?)?;
            let o10_from_iso = isoformat.replacen('T', " ", 1);
            assert_eq!(o10_from_iso, o10, "str(datetime) {id}");
        }
        compared += 1;
    }

    assert!(compared > 0, "no UTC timestamp vectors were compared");
    println!(
        "GV-12 timestamp vectors: {compared} compared, {skipped} not representable (non-UTC or naive)"
    );
    Ok(())
}

#[test]
fn gv12_model_datetime_uses_z_suffix() -> TestResult {
    // Pydantic's JSON form of a model datetime, from the `model-inner-only` vector.
    let golden = load("jsonfmt/cases.json")?;
    let case = golden["cases"]
        .as_array()
        .ok_or("cases is not an array")?
        .iter()
        .find(|case| case["id"] == "model-inner-only")
        .ok_or("model-inner-only vector missing")?;
    let o01: Value = serde_json::from_str(str_field(case, &["outputs", "O01", "output"])?)?;
    let at = o01["at"].as_str().ok_or("at is not a string")?;
    let stamp = Timestamp::parse_iso("2026-02-28T23:59:59.999999+00:00")?;
    assert_eq!(stamp.format_z(), at);
    assert_eq!(
        Timestamp::parse_iso("2026-10-25T13:56:00+00:00")?.format_z(),
        "2026-10-25T13:56:00Z"
    );
    Ok(())
}

#[test]
fn gv01_state_names_are_the_enum_wire_strings() -> TestResult {
    let constants = load("lifecycle/constants.json")?;

    let task_keys: Vec<&str> = constants["task_transitions"]
        .as_object()
        .ok_or("task_transitions is not an object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(task_keys.len(), TaskStatus::ALL.len(), "task status count");
    for name in &task_keys {
        assert!(
            TaskStatus::parse(name).is_some(),
            "unknown task status {name}"
        );
    }
    for name in constants["requires_dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            constants["restricted_targets"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .chain(
            constants["tracker_close_targets"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .filter_map(Value::as_str)
    {
        assert!(
            TaskStatus::parse(name).is_some(),
            "unknown task status {name}"
        );
    }

    let attempt_keys: Vec<&str> = constants["attempt_transitions"]
        .as_object()
        .ok_or("attempt_transitions is not an object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        attempt_keys.len(),
        AttemptStatus::ALL.len(),
        "attempt status count"
    );
    for name in &attempt_keys {
        assert!(
            AttemptStatus::parse(name).is_some(),
            "unknown attempt status {name}"
        );
    }
    Ok(())
}
