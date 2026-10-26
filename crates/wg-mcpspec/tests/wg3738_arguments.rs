//! WG-3738: argument validation errors against what Pydantic itself prints.
//!
//! `golden/argument_errors.json` was produced by validating the arguments with the argument model
//! the Python SDK builds for each tool (`fn_metadata.validate_arguments`) and keeping
//! `str(ValidationError)`. It pins the `input_value` elision rule (a repr longer than 50 UTF-8
//! bytes keeps 25 bytes of head and 24 of tail, cut at character boundaries) and the error order
//! of a multi-error message (the model's field order, not the order of the arguments).

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use serde_json::Value;
use wg_mcpspec::{Catalog, check_arguments};

fn golden() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/argument_errors.json");
    let text = std::fs::read_to_string(path).expect("golden");
    serde_json::from_str::<Vec<Value>>(&text).expect("json")
}

#[test]
fn argument_errors_equal_the_text_pydantic_prints() {
    let catalog = Catalog::embedded().expect("catalog");
    let mut failures = Vec::new();
    for case in golden() {
        let tool = case["tool"].as_str().expect("tool");
        let spec = catalog.tool(tool).expect("tool in catalog");
        let outcome = check_arguments(spec, Some(&case["arguments"]));
        match (case["text"].as_str(), outcome) {
            (None, Ok(_)) => {}
            (Some(wanted), Err(err)) if err.text() == wanted => {}
            (Some(wanted), Err(err)) => failures.push(format!(
                "{tool} {}\n--- python\n{wanted}\n--- rust\n{}",
                case["arguments"],
                err.text()
            )),
            (wanted, outcome) => failures.push(format!(
                "{tool} {}: python {:?}, rust {}",
                case["arguments"],
                wanted.map(|_| "an error"),
                if outcome.is_ok() { "valid" } else { "an error" }
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn argument_order_lists_every_property_of_every_tool() {
    let order: Value =
        serde_json::from_str(include_str!("../src/argument_order.json")).expect("order");
    let catalog = Catalog::embedded().expect("catalog");
    assert_eq!(
        order["tools"].as_object().expect("tools").len(),
        catalog.tools.len()
    );
    for spec in &catalog.tools {
        let mut declared: Vec<&str> = order["tools"][&spec.name]
            .as_array()
            .unwrap_or_else(|| panic!("{} has no declared order", spec.name))
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let properties = &spec.descriptor["inputSchema"]["properties"];
        let mut wire: Vec<&str> = properties
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect();
        declared.sort_unstable();
        wire.sort_unstable();
        assert_eq!(declared, wire, "{}", spec.name);
    }
}
