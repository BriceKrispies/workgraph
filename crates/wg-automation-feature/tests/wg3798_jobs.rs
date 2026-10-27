//! The automation job handler and maintenance steps (WG-3798): the pure decisions an action run
//! makes, and what the feature hands the worker.

use serde_json::{Map, Value, json};
use wg_automation_feature::spec::execute as exec;
use wg_jobs::maintenance::Step;
use wg_json::{PyStr, Value as Wire};
use wg_runtime::HandlerRegistry;
use wg_types::Permission;

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn only_runs_of_human_rules_may_start_runs() {
    assert!(exec::run_permissions("human:brice").contains(&Permission::RunStart));
    assert!(!exec::run_permissions("agent:claude").contains(&Permission::RunStart));
    assert!(exec::run_permissions("agent:claude").contains(&Permission::TaskWrite));
}

#[test]
fn an_update_task_body_keeps_the_fields_that_are_set_and_names_the_automation() {
    let action = object(json!({
        "type": "update_task",
        "priority": "high",
        "status": null,
        "assignee": null,
    }));
    assert_eq!(
        Value::Object(exec::update_body(&action)),
        json!({"priority": "high", "reason": "automation"})
    );
}

#[test]
fn chained_children_depend_on_the_previous_child_and_default_their_priority() {
    let child = object(json!({"title": "child of {{task.key}}", "tags": ["a"]}));
    let context = json!({"task": {"id": "abc", "key": "WG-1"}});
    let body = exec::child_body(&child, "abc", &["r1".to_owned()], Some("WG-2"), &context);
    assert_eq!(body["title"], json!("child of WG-1"));
    assert_eq!(body["priority"], json!("medium"));
    assert_eq!(body["depends_on"], json!(["WG-2"]));
    assert_eq!(body["repositories"], json!(["r1"]));
    assert_eq!(body["status"], json!("ready"));
}

#[test]
fn a_dependency_action_reads_either_direction() {
    let depends = object(json!({"type": "create_dependency", "depends_on": "WG-9"}));
    assert_eq!(
        exec::dependency_request(&depends, "WG-1", &json!({})),
        ("WG-1".to_owned(), "WG-9".to_owned())
    );
    let blocks = object(json!({"type": "create_dependency", "blocks": "WG-9"}));
    assert_eq!(
        exec::dependency_request(&blocks, "WG-1", &json!({})),
        ("WG-9".to_owned(), "WG-1".to_owned())
    );
}

#[test]
fn wire_values_read_back_as_json_for_the_action_outputs() {
    let wire = Wire::Dict(vec![
        (PyStr::from("key"), Wire::Str(PyStr::from("WG-1"))),
        (
            PyStr::from("tags"),
            Wire::List(vec![Wire::Str(PyStr::from("a"))]),
        ),
    ]);
    assert_eq!(exec::to_json(&wire), json!({"key": "WG-1", "tags": ["a"]}));
}

#[test]
fn the_feature_hands_the_worker_its_job_and_its_two_maintenance_steps() {
    let registry = HandlerRegistry::from_features(&[&wg_automation_feature::FEATURE]);
    assert!(registry.handlers.contains_key("automation_action"));
    let slots: Vec<Step> = registry.maintenance.iter().map(|step| step.slot).collect();
    assert_eq!(
        slots,
        vec![Step::ProcessAutomationEvents, Step::RunScheduledRules]
    );
}
