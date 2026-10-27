//! WG-3853: the hook spool replay maintenance step. The feature fills the `ReplayHooks` slot, and
//! the step answers the counts of Python's `replay_spool`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;
use wg_jobs::maintenance::Step;
use wg_runtime::{App, system_context};
use wg_telemetry_feature::jobs::MAINTENANCE;

#[test]
fn the_feature_fills_the_replay_hooks_slot() {
    let slots: Vec<Step> = MAINTENANCE.iter().map(|step| step.slot).collect();
    assert_eq!(slots, vec![Step::ReplayHooks]);
}

#[test]
fn replaying_an_empty_spool_reports_no_records() {
    let dir = std::env::temp_dir().join(format!("wg3853-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::open(Some(&dir)).expect("the app opens");
    let context = system_context("maintenance").expect("the system context");
    let report = (MAINTENANCE[0].run)(&mut app, &context).expect("the replay runs");
    assert_eq!(report, json!({"ingested": 0, "duplicate": 0, "failed": 0}));
}
