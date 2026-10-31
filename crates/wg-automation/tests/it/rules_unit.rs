//! Rule-level decisions of wg-automation: trigger validation, action descriptions, the depth and
//! rate guards, and the precedence between validation errors.

use serde_json::{Map, Value, json};
use wg_automation::condition::{ConditionError, validate};
use wg_automation::guard::{
    RunDecision, decide_run, depth_limit, event_depth, rate_limited, run_depth,
};
use wg_automation::rules::{describe_action, validate_trigger};

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

#[test]
fn event_trigger_needs_known_events() {
    assert!(
        validate_trigger(&object(
            json!({"type": "event", "event_types": ["task.created"]})
        ))
        .is_ok()
    );
    assert!(validate_trigger(&object(json!({"type": "event", "event_types": []}))).is_err());
    assert!(
        validate_trigger(&object(
            json!({"type": "event", "event_types": ["task.exploded"]})
        ))
        .is_err()
    );
}

#[test]
fn schedule_trigger_needs_exactly_one_of_cron_or_interval() {
    assert!(validate_trigger(&object(json!({"type": "schedule", "cron": "*/5 * * * *"}))).is_ok());
    assert!(
        validate_trigger(&object(
            json!({"type": "schedule", "interval_seconds": 120})
        ))
        .is_ok()
    );
    assert!(validate_trigger(&object(json!({"type": "schedule"}))).is_err());
    assert!(
        validate_trigger(&object(
            json!({"type": "schedule", "cron": "* * * * *", "interval_seconds": 120})
        ))
        .is_err()
    );
    assert!(validate_trigger(&object(json!({"type": "schedule", "cron": "* * * *"}))).is_err());
    assert!(
        validate_trigger(&object(json!({"type": "schedule", "interval_seconds": 30}))).is_err()
    );
}

#[test]
fn manual_trigger_ignores_schedule_fields() {
    assert!(validate_trigger(&object(json!({"type": "manual"}))).is_ok());
    // As in Python, only schedule triggers parse their cron expression.
    assert!(validate_trigger(&object(json!({"type": "manual", "cron": "bad cron"}))).is_ok());
}

#[test]
fn update_task_is_described_with_its_changes() {
    let action = object(json!({"type": "update_task", "priority": "high", "status": null}));
    let context = json!({"task": {"key": "WG-7"}});
    assert_eq!(
        describe_action(&action, &context),
        "update WG-7: priority=high, status=None"
    );
}

#[test]
fn notify_renders_its_title_from_the_context() {
    let action = object(json!({"type": "notify", "title": "New bug {{task.key}}"}));
    let context = json!({"task": {"key": "WG-12"}});
    assert_eq!(describe_action(&action, &context), "notify: New bug WG-12");
}

#[test]
fn missing_task_falls_back_to_the_generic_phrase() {
    let action = object(json!({"type": "run_verification"}));
    assert_eq!(
        describe_action(&action, &json!({})),
        "run verification checks for the triggering task"
    );
}

#[test]
fn event_depth_is_read_from_the_automation_metadata() {
    assert_eq!(event_depth(&json!({"_automation": {"depth": 2}})), 2);
    assert_eq!(event_depth(&json!({"_automation": "nope"})), 0);
    assert_eq!(event_depth(&json!({})), 0);
    assert_eq!(
        run_depth(event_depth(&json!({"_automation": {"depth": 2}}))),
        3
    );
}

#[test]
fn depth_limit_is_the_smaller_maximum() {
    assert_eq!(depth_limit(3, 5), 3);
    assert_eq!(depth_limit(7, 2), 2);
}

#[test]
fn rate_budget_is_thirty_runs_a_minute() {
    assert!(!rate_limited(29));
    assert!(rate_limited(30));
}

#[test]
fn depth_guard_beats_rate_guard_and_dry_runs_skip_the_rate_guard() {
    let depth_first = decide_run(4, 3, false, true, || true);
    assert_eq!(
        depth_first,
        RunDecision::Skipped("automation depth 4 exceeds limit 3 (recursion guard)".to_owned())
    );
    let limited = decide_run(1, 3, false, true, || true);
    assert_eq!(limited.status(), "skipped");
    assert_eq!(
        decide_run(1, 3, true, true, || true),
        RunDecision::Previewed
    );
    assert_eq!(
        decide_run(1, 3, false, false, || false),
        RunDecision::NotMatched
    );
    assert_eq!(decide_run(1, 3, false, false, || true), RunDecision::Queued);
}

#[test]
fn a_bad_pattern_beats_a_shape_error() {
    let mixed = json!({"all": [{"field": "a", "op": "matches", "value": "[unclosed"}], "bogus": 1});
    assert!(matches!(
        validate(&mixed),
        Err(ConditionError::BadPattern(_))
    ));
    let shape_only = json!({"field": "a", "op": "like", "value": "x"});
    assert!(matches!(
        validate(&shape_only),
        Err(ConditionError::Invalid(_))
    ));
}
