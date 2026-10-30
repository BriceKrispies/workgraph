//! GV-10: every case of `golden/automation/{cron,templates,conditions}.json` must match the
//! cron, template and condition code.
//!
//! The golden files were recorded by `tools/golden/gen_automation.py` from the Python
//! implementation. Each case is rebuilt here in the same JSON shape and compared as a whole.

use serde_json::{Value, json};
use wg_automation::condition::{ConditionError, OPERATORS, TraceEntry, evaluate, validate};
use wg_automation::cron::Cron;
use wg_automation::template::render;
use wg_types::Timestamp;

fn golden(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/automation/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {path}: {err}"));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parsing {path}: {err}"))
}

fn cases(name: &str) -> Vec<Value> {
    golden(name)["cases"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("{name} has no cases"))
}

fn moment(text: &str) -> Timestamp {
    Timestamp::parse_iso(text).unwrap_or_else(|err| panic!("bad moment {text}: {err:?}"))
}

fn cron_case(case: &Value) -> Value {
    let expression = case["expression"].as_str().unwrap_or_default();
    let parsed = match Cron::parse(expression) {
        Ok(cron) => cron,
        Err(err) => {
            return json!({
                "id": case["id"],
                "expression": expression,
                "parse": { "ok": false, "error": err.message },
                "references": [],
                "matches": [],
            });
        }
    };
    let references: Vec<Value> = case["references"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let reference = entry["reference"].as_str().unwrap_or_default();
            match parsed.next_after(moment(reference).to_offset_datetime()) {
                Ok(next) => json!({
                    "reference": reference,
                    "next_after": Timestamp::from_offset_datetime(next)
                        .map(Timestamp::format_isoformat)
                        .unwrap_or_default(),
                    "error": null,
                }),
                Err(err) => json!({
                    "reference": reference,
                    "next_after": null,
                    "error": err.message,
                }),
            }
        })
        .collect();
    let matches: Vec<Value> = case["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let text = entry["moment"].as_str().unwrap_or_default();
            json!({
                "moment": text,
                "result": parsed.matches(moment(text).to_offset_datetime()),
            })
        })
        .collect();
    json!({
        "id": case["id"],
        "expression": expression,
        "parse": { "ok": true, "error": null },
        "references": references,
        "matches": matches,
    })
}

#[test]
fn cron_matches_golden() {
    for case in cases("cron") {
        assert_eq!(cron_case(&case), case, "cron case {}", case["id"]);
    }
}

#[test]
fn templates_match_golden() {
    for case in cases("templates") {
        let limit = case["limit"].as_u64().map_or(500, |value| value as usize);
        let output = render(
            case["template"].as_str().unwrap_or_default(),
            &case["context"],
            limit,
        );
        assert_eq!(
            json!(output),
            case["output"],
            "template case {}",
            case["id"]
        );
    }
}

/// The golden `error` class of a validation failure: a Pydantic `ValidationError`, or the
/// `re.error` a bad `matches` pattern raises.
fn error_class(error: &ConditionError) -> &'static str {
    match error {
        ConditionError::Invalid(_) => "ValidationError",
        ConditionError::BadPattern(_) => "error",
    }
}

fn trace_json(trace: &[TraceEntry]) -> Value {
    Value::Array(trace.iter().map(TraceEntry::to_json).collect())
}

fn condition_case(case: &Value) -> Value {
    let condition_json = &case["condition"];
    let context = &case["context"];
    let id = &case["id"];
    let (condition, valid, error) = if condition_json.is_null() {
        (None, true, Value::Null)
    } else {
        match validate(condition_json) {
            Ok(condition) => (Some(condition), true, Value::Null),
            Err(err) => (None, false, json!(error_class(&err))),
        }
    };
    if !valid {
        return json!({
            "id": id,
            "condition": condition_json,
            "context": context,
            "valid": false,
            "error": error,
            "result": null,
            "trace": null,
        });
    }
    let (result, trace) = evaluate(condition.as_ref(), context);
    json!({
        "id": id,
        "condition": condition_json,
        "context": context,
        "valid": true,
        "error": null,
        "result": result,
        "trace": trace_json(&trace),
    })
}

#[test]
fn conditions_match_golden() {
    for case in cases("conditions") {
        assert_eq!(condition_case(&case), case, "condition case {}", case["id"]);
    }
}

#[test]
fn operator_list_matches_golden() {
    let document = golden("conditions");
    let recorded: Vec<&str> = document["operators"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let ours: Vec<&str> = OPERATORS.iter().map(|op| op.name()).collect();
    assert_eq!(ours, recorded);
}
