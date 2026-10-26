//! IF-30: the pure half of the MCP server against the recorded stdio transcript
//! (`contracts/mcp/stdio.jsonl`).

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use serde_json::{Value, json};
use wg_mcpspec::{Catalog, check_arguments, error_result, execution_error_text, unknown_tool_text};

fn transcript() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/mcp/stdio.jsonl");
    std::fs::read_to_string(path)
        .expect("transcript")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect()
}

fn result_of(label: &str) -> Value {
    transcript()
        .into_iter()
        .find(|row| row["label"] == label)
        .expect("exchange")["response"]["result"]
        .clone()
}

#[test]
fn listings_equal_the_transcript() {
    let catalog = Catalog::embedded().expect("catalog");
    assert_eq!(catalog.tools_list(), result_of("tools/list"));
    assert_eq!(catalog.resources_list(), result_of("resources/list"));
    assert_eq!(
        catalog.resource_templates_list(),
        result_of("resources/templates/list")
    );
    // Order is part of the reply.
    let recorded = result_of("tools/list");
    let names: Vec<&str> = catalog.tools.iter().map(|t| t.name.as_str()).collect();
    let wanted: Vec<&str> = recorded["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, wanted);
}

#[test]
fn unknown_tool_result_equals_the_transcript() {
    let wanted = result_of("tools/call:unknown");
    assert_eq!(error_result(&unknown_tool_text("no_such_tool")), wanted);
}

#[test]
fn invalid_arguments_result_equals_the_transcript() {
    let catalog = Catalog::embedded().expect("catalog");
    let spec = catalog.tool("task_get").expect("task_get");
    let err = check_arguments(spec, Some(&json!({"task": 5}))).expect_err("invalid");
    assert_eq!(
        error_result(&execution_error_text("task_get", &err.text())),
        result_of("tools/call:invalid-args")
    );
}

#[test]
fn missing_and_valid_arguments() {
    let catalog = Catalog::embedded().expect("catalog");
    let spec = catalog.tool("task_get").expect("task_get");
    let err = check_arguments(spec, None).expect_err("missing");
    assert_eq!(
        err.text(),
        "1 validation error for task_getArguments\ntask\n  Field required [type=missing, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing"
    );
    let ok = check_arguments(spec, Some(&json!({"task": "WG-1"}))).expect("valid");
    assert_eq!(ok["task"], "WG-1");
    assert!(ok["workspace"].is_null());
}

/// Every tool's schema loads into a model, and every error-kind exchange of the transcript
/// names a registered tool.
#[test]
fn every_recorded_tool_is_registered() {
    let catalog = Catalog::embedded().expect("catalog");
    assert_eq!(catalog.tools.len(), 69);
    for row in transcript() {
        if let Some(name) = row["tool"].as_str()
            && name != "no_such_tool"
        {
            assert!(catalog.tool(name).is_some(), "{name}");
        }
    }
}
