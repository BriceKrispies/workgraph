//! The rule document validator (`RuleSpec.model_validate` and its dump), on the documents the
//! automation commands take. The error locations are the ones Pydantic reports.

#![allow(clippy::expect_used)]

use serde_json::{Value, json};
use wg_automation::spec::{Failure, validate_rule};

fn refused(document: &Value) -> Vec<Value> {
    match validate_rule(document) {
        Err(Failure::Invalid(issues)) => issues.iter().map(|issue| issue.to_json()).collect(),
        other => panic!("expected the document to be refused, got {other:?}"),
    }
}

// covers: C026
#[test]
fn a_valid_rule_is_dumped_in_the_column_forms() {
    let spec = validate_rule(&json!({
        "name": "tag-high",
        "trigger": {"type": "event", "event_types": ["task.created"]},
        "actions": [{"type": "update_task", "priority": "low"}, {"type": "notify", "title": "hi"}],
        "conditions": {"field": "task.priority", "op": "eq", "value": "high"},
    }))
    .expect("the rule is valid");
    assert_eq!(spec.name, "tag-high");
    assert_eq!(spec.description, "");
    assert!(spec.enabled);
    assert_eq!(spec.retry_max_attempts, 3);
    assert_eq!(spec.max_depth, 3);
    assert_eq!(
        Value::Object(spec.trigger.clone()),
        json!({"type": "event", "event_types": ["task.created"]})
    );
    // The shorthand predicate becomes `predicate`, and an unset action field is left out.
    assert_eq!(
        Value::Object(spec.conditions.clone()),
        json!({"predicate": {"field": "task.priority", "op": "eq", "value": "high"}})
    );
    assert_eq!(
        Value::Array(spec.actions.iter().cloned().map(Value::Object).collect()),
        json!([
            {"type": "update_task", "priority": "low"},
            {"type": "notify", "level": "info", "title": "hi", "body": ""},
        ])
    );
}

// covers: C026
#[test]
fn an_event_trigger_needs_a_known_event_type() {
    let errors = refused(&json!({
        "name": "x",
        "trigger": {"type": "event", "event_types": ["task.exploded"]},
        "actions": [{"type": "classify_task"}],
    }));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["type"], "value_error");
    assert_eq!(errors[0]["loc"], json!(["trigger"]));
}

// covers: C026
#[test]
fn empty_actions_and_a_bare_event_trigger_are_both_reported() {
    let errors = refused(&json!({
        "name": "bad",
        "trigger": {"type": "event"},
        "actions": [],
    }));
    let kinds: Vec<(&str, Value)> = errors
        .iter()
        .map(|error| {
            (
                error["type"].as_str().unwrap_or_default(),
                error["loc"].clone(),
            )
        })
        .collect();
    assert!(kinds.contains(&("value_error", json!(["trigger"]))));
    assert!(kinds.contains(&("too_short", json!(["actions"]))));
}

// covers: C036
#[test]
fn a_dependency_names_exactly_one_target() {
    let errors = refused(&json!({
        "name": "dep",
        "trigger": {"type": "manual"},
        "actions": [{"type": "create_dependency", "depends_on": "WG-1", "blocks": "WG-2"}],
    }));
    assert_eq!(errors[0]["loc"], json!(["actions", 0, "create_dependency"]));
    assert_eq!(
        errors[0]["msg"],
        "Value error, specify exactly one of depends_on or blocks"
    );
}

// covers: C026
#[test]
fn a_cron_expression_that_does_not_parse_is_raised_not_wrapped() {
    match validate_rule(&json!({
        "name": "tick",
        "trigger": {"type": "schedule", "cron": "not a cron"},
        "actions": [{"type": "classify_task"}],
    })) {
        Err(Failure::Raised(error)) => {
            assert_eq!(error.code.as_str(), "validation_failed");
            assert!(error.message.contains("cron"));
        }
        other => panic!("expected a raised error, got {other:?}"),
    }
}
