//! GV-13: every case of `golden/rich/*.json` must produce the recorded stdout, stderr and (for
//! malformed markup) `MarkupError` message at the recorded console width.

use serde_json::Value as Json;
use wg_render::{Console, Printed};

fn load(name: &str) -> Vec<Json> {
    let path = format!(
        "{}/../../golden/rich/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let doc: Json = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    doc["cases"].as_array().cloned().unwrap_or_default()
}

fn width(case: &Json) -> usize {
    usize::try_from(case["width"].as_u64().unwrap_or(0)).unwrap_or(0)
}

/// The title the generator passed: the case's own, or `Markup` for the markup cases.
fn title(case: &Json) -> Option<&str> {
    if case["id"]
        .as_str()
        .unwrap_or_default()
        .starts_with("markup-table")
    {
        return Some("Markup");
    }
    case["title"].as_str()
}

/// A cell as Python's `"" if v is None else str(v)`.
fn cell(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn rows(case: &Json) -> Vec<Vec<String>> {
    case["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .map(|r| r.iter().map(cell).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn headers(case: &Json) -> Vec<String> {
    case["headers"]
        .as_array()
        .map(|h| h.iter().map(cell).collect())
        .unwrap_or_default()
}

fn table(case: &Json) -> Printed {
    let headers = headers(case);
    let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    Console::new(width(case)).table(&rows(case), &refs, title(case))
}

/// The vectors store the empty list and dict as the strings `[]` and `{}`; `common.kv` skips them
/// along with `None`.
fn pairs(case: &Json) -> Vec<(String, Option<String>)> {
    case["pairs"]
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .map(|pair| {
                    let key = pair[0].as_str().unwrap_or_default().to_owned();
                    let value = match &pair[1] {
                        Json::Null => None,
                        Json::String(s) if s == "[]" || s == "{}" => None,
                        other => Some(cell(other)),
                    };
                    (key, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn kv(case: &Json) -> Printed {
    let owned = pairs(case);
    let refs: Vec<(&str, Option<&str>)> = owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect();
    Console::new(width(case)).kv(&refs, title(case))
}

/// Checks `printed` against the case's recorded `output`; `stream` names where the text goes.
fn check(case: &Json, printed: &Printed, stream: &str) {
    let id = case["id"].as_str().unwrap_or("?");
    let output = &case["output"];
    let other = if stream == "stdout" {
        "stderr"
    } else {
        "stdout"
    };
    assert_eq!(
        printed.text,
        output[stream].as_str().unwrap_or_default(),
        "{id}: {stream}"
    );
    assert_eq!(
        output[other].as_str().unwrap_or_default(),
        "",
        "{id}: {other}"
    );
    let expected_error = output["exception"].as_str();
    let got_error = printed.error.as_ref().map(|e| format!("MarkupError: {e}"));
    assert_eq!(got_error.as_deref(), expected_error, "{id}: exception");
}

#[test]
fn tables() {
    let cases = load("tables");
    assert_eq!(cases.len(), 64);
    for case in &cases {
        check(case, &table(case), "stdout");
    }
}

#[test]
fn empty_tables() {
    let cases = load("empty_tables");
    assert_eq!(cases.len(), 64);
    for case in &cases {
        check(case, &table(case), "stdout");
    }
}

#[test]
fn key_value_blocks() {
    let cases = load("kv");
    assert_eq!(cases.len(), 8);
    for case in &cases {
        check(case, &kv(case), "stdout");
    }
}

#[test]
fn markup() {
    let cases = load("markup");
    assert_eq!(cases.len(), 132);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        if id.starts_with("markup-table") {
            check(case, &table(case), "stdout");
        } else if id.starts_with("markup-kv") {
            check(case, &kv(case), "stdout");
        } else {
            let text = case["text"].as_str().unwrap_or_default();
            check(case, &Console::new(width(case)).fail(text), "stderr");
        }
    }
}

fn detail_text(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The vector files have sorted keys, so the order the details were printed in is not recorded
/// in `details`. Order them by where each `  key: ` line appears in the recorded stderr; a key
/// that markup swallowed (so it never appears) sorts last.
fn details_in_printed_order(case: &Json) -> Vec<(String, String)> {
    let stderr = case["output"]["stderr"].as_str().unwrap_or_default();
    let mut details: Vec<(String, String)> = case["details"]
        .as_object()
        .map(|d| d.iter().map(|(k, v)| (k.clone(), detail_text(v))).collect())
        .unwrap_or_default();
    details.sort_by_key(|(key, _)| stderr.find(&format!("\n  {key}: ")).unwrap_or(usize::MAX));
    details
}

#[test]
fn error_reports() {
    let cases = load("errors");
    assert_eq!(cases.len(), 29);
    let mut checked = 0;
    for case in &cases {
        // The JSON-mode case writes the error envelope, which wg-json owns.
        let Some(code) = case["code"].as_str() else {
            continue;
        };
        let details = details_in_printed_order(case);
        let refs: Vec<(&str, &str)> = details
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let message = case["message"].as_str().unwrap_or_default();
        let printed = Console::new(width(case)).error_report(code, message, &refs);
        check(case, &printed, "stderr");
        checked += 1;
    }
    assert_eq!(checked, 28);
}

/// Consoles narrower than the vectors go must still draw something, never panic.
#[test]
fn narrow_consoles_do_not_panic() {
    let cases = load("tables");
    for case in cases.iter().filter(|c| c["width"] == 40) {
        let headers = headers(case);
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        for width in 0..40 {
            let printed = Console::new(width).table(&rows(case), &refs, title(case));
            assert!(printed.error.is_none());
        }
    }
    for width in 0..10 {
        let console = Console::new(width);
        assert!(
            console
                .print("a b\tc 数据库 🚀 xxxxxxxxxxxxx")
                .error
                .is_none()
        );
    }
}
