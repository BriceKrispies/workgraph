//! Scheduling decisions of wg-jobs: the automation schedule step, the agent scheduler's fair
//! order and capacity.

use serde_json::{Map, Value, json};
use wg_jobs::scheduler::{Candidate, ScheduleStep, capacity, fair_order, schedule_step};
use wg_types::Timestamp;

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn at(text: &str) -> Timestamp {
    Timestamp::parse_iso(text).unwrap_or_else(|err| panic!("bad timestamp {text}: {err:?}"))
}

#[test]
fn candidates_interleave_across_projects_within_a_rank() {
    let candidate = |project: &str, rank: i64, item: &str| Candidate {
        project: Some(project.to_owned()),
        rank,
        item: item.to_owned(),
    };
    let ordered = fair_order(vec![
        candidate("alpha", 1, "a1"),
        candidate("alpha", 1, "a2"),
        candidate("beta", 1, "b1"),
        candidate("beta", 2, "b-low"),
    ]);
    assert_eq!(ordered, vec!["a1", "b1", "a2", "b-low"]);
}

#[test]
fn capacity_never_goes_negative() {
    assert_eq!(capacity(4, 1), 3);
    assert_eq!(capacity(2, 5), 0);
}

#[test]
fn a_schedule_is_initialised_then_waits_then_fires_once_due() {
    let trigger = object(json!({"type": "schedule", "cron": "*/15 * * * *"}));
    let initialised = schedule_step(&trigger, None, at("2026-10-25T10:02:30.5+00:00")).ok();
    assert_eq!(
        initialised,
        Some(ScheduleStep::Initialise {
            next_run_at: at("2026-10-25T10:15:00+00:00"),
        })
    );
    let waiting = schedule_step(
        &trigger,
        Some(at("2026-10-25T10:15:00+00:00")),
        at("2026-10-25T10:14:59+00:00"),
    )
    .ok();
    assert_eq!(waiting, Some(ScheduleStep::Wait));
    let fired = schedule_step(
        &trigger,
        Some(at("2026-10-25T10:15:00.250000+00:00")),
        at("2026-10-25T10:15:20+00:00"),
    )
    .ok();
    let Some(ScheduleStep::Fire { fire, next_run_at }) = fired else {
        panic!("expected a fire, got {fired:?}");
    };
    assert_eq!(fire.trigger_key, "schedule:2026-10-25T10:15:00+00:00");
    assert_eq!(fire.scheduled_for, "2026-10-25T10:15:00+00:00");
    assert_eq!(next_run_at, at("2026-10-25T10:30:00+00:00"));
}

#[test]
fn non_schedule_rules_are_left_alone() {
    let trigger = object(json!({"type": "event", "event_types": ["task.created"]}));
    let step = schedule_step(&trigger, None, at("2026-10-25T10:00:00+00:00")).ok();
    assert_eq!(step, Some(ScheduleStep::Idle));
}
