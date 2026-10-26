#![allow(clippy::expect_used)]
//! CORE-29, subagents and agent time: the cases of `tests/integration/test_subagent_report.py` and
//! `tests/integration/test_agent_time_report.py`, ported to the pure reports. Each fixture is the
//! seed of the Python test, with the same tasks, commits, acceptances and runs, and each assertion
//! is the Python assertion.

use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_analytics::agent_time::{self, Group};
use wg_analytics::common::{Scope, Window};
use wg_analytics::source::{
    AcceptanceRow, AnalyticsInput, CommitLink, CommitRow, SubagentRunRow, Surface, TaskRow,
};
use wg_analytics::subagents::{self, Period};
use wg_experiments::stats::{Z_95, wilson};
use wg_types::Timestamp;

const HAIKU: &str = "claude-haiku-5-5";
const SONNET: &str = "claude-sonnet-5-5";

fn ts(text: &str) -> Timestamp {
    Timestamp::parse_iso(text).expect("a valid timestamp")
}

/// `_at(day, hour)`: UTC noon by default in the Python tests.
fn day_at(day: &str, hour: u32) -> Timestamp {
    ts(&format!("{day}T{hour:02}:00:00Z"))
}

fn hm(hour: u32, minute: u32) -> Timestamp {
    ts(&format!("2026-10-14T{hour:02}:{minute:02}:00Z"))
}

fn workspace_scope() -> Scope {
    Scope {
        workspace_id: "W1".to_owned(),
        default_workspace_id: Some("W1".to_owned()),
    }
}

fn task(id: &str, status: &str) -> TaskRow {
    TaskRow {
        id: Some(id.to_owned()),
        key: Some(format!("WG-{id}")),
        workspace_id: Some("W1".to_owned()),
        status: Some(status.to_owned()),
        ..Default::default()
    }
}

fn commit(id: &str, insertions: i64, deletions: i64, merge: bool) -> CommitRow {
    CommitRow {
        id: Some(id.to_owned()),
        insertions: Some(insertions),
        deletions: Some(deletions),
        is_merge: Some(merge),
        ..Default::default()
    }
}

fn link(commit: &str, task: &str) -> CommitLink {
    CommitLink {
        commit_id: Some(commit.to_owned()),
        task_id: Some(task.to_owned()),
        confidence: Some(1.0),
    }
}

fn acceptance(task: &str, decision: &str) -> AcceptanceRow {
    AcceptanceRow {
        task_id: Some(task.to_owned()),
        decision: Some(decision.to_owned()),
    }
}

/// A stopped (or open) subagent run, with its telemetry and usage objects.
fn run(
    stopped: Option<Timestamp>,
    task: Option<&str>,
    model: Option<&str>,
    requested: Option<&str>,
    telemetry: Value,
    usage: Value,
) -> SubagentRunRow {
    SubagentRunRow {
        task_id: task.map(str::to_owned),
        model: model.map(str::to_owned),
        requested_model: requested.map(str::to_owned),
        stopped_at: stopped,
        usage: Some(usage.to_string()),
        telemetry: Some(telemetry.to_string()),
        ..Default::default()
    }
}

/// `_seed` of `test_subagent_report.py`.
fn subagent_seed() -> AnalyticsInput {
    let tasks: VectorSync<TaskRow> = [
        task("acc1", "accepted"),
        task("acc2", "accepted"),
        task("rej", "awaiting_review"),
        task("fail", "failed"),
        task("pend", "running"),
    ]
    .into_iter()
    .collect();
    let commits: VectorSync<CommitRow> = [
        commit("c-acc1-a", 10, 3, false),
        commit("c-acc1-b", 5, 2, false),
        commit("c-acc1-merge", 500, 0, true),
        commit("c-acc2", 1000, 0, false),
        commit("c-rej", 4, 0, false),
    ]
    .into_iter()
    .collect();
    let links: VectorSync<CommitLink> = [
        link("c-acc1-a", "acc1"),
        link("c-acc1-b", "acc1"),
        link("c-acc1-merge", "acc1"),
        link("c-acc2", "acc2"),
        link("c-rej", "rej"),
    ]
    .into_iter()
    .collect();
    let surfaces: VectorSync<Surface> = [Surface {
        task_id: Some("acc2".to_owned()),
        insertions: Some(100),
        deletions: Some(20),
        ..Default::default()
    }]
    .into_iter()
    .collect();
    let acceptances: VectorSync<AcceptanceRow> = [
        acceptance("acc1", "accepted"),
        acceptance("rej", "rejected"),
    ]
    .into_iter()
    .collect();
    let out = |tokens: i64| json!({"output_tokens": tokens});
    let turns = |turns: i64, errors: i64| json!({"tool_errors": errors, "turns": turns});
    let runs: VectorSync<SubagentRunRow> = [
        run(
            Some(day_at("2026-10-05", 12)),
            Some("acc1"),
            Some(HAIKU),
            None,
            turns(10, 1),
            out(1000),
        ),
        run(
            Some(day_at("2026-10-06", 12)),
            Some("acc1"),
            None,
            None,
            json!({"tool_errors": 0}),
            json!({}),
        ),
        run(
            Some(day_at("2026-10-07", 12)),
            Some("acc2"),
            Some(HAIKU),
            None,
            turns(20, 0),
            out(2000),
        ),
        run(
            Some(day_at("2026-10-13", 0)),
            Some("acc2"),
            Some(HAIKU),
            None,
            turns(40, 0),
            out(4000),
        ),
        run(
            Some(day_at("2026-10-12", 12)),
            Some("rej"),
            Some(SONNET),
            None,
            turns(30, 2),
            out(3000),
        ),
        run(
            Some(day_at("2026-09-30", 12)),
            Some("fail"),
            None,
            Some(HAIKU),
            json!({"tool_errors": 0}),
            json!({}),
        ),
        run(
            Some(day_at("2026-10-14", 12)),
            Some("pend"),
            Some(SONNET),
            None,
            json!({"tool_errors": 0}),
            json!({}),
        ),
        run(
            Some(day_at("2026-10-14", 12)),
            None,
            Some(HAIKU),
            None,
            json!({"tool_errors": 0}),
            out(500),
        ),
        run(
            None,
            Some("pend"),
            Some(HAIKU),
            None,
            json!({"tool_errors": 0}),
            out(9999),
        ),
        run(
            Some(day_at("2026-10-14", 12)),
            Some("pend"),
            Some(HAIKU),
            None,
            json!({"tool_errors": 0, "trial_ids": ["trial-1"]}),
            out(7777),
        ),
    ]
    .into_iter()
    .collect();
    AnalyticsInput {
        tasks,
        commits,
        links,
        surfaces,
        acceptances,
        subagent_runs: runs,
        ..Default::default()
    }
}

/// The row of `rows` whose fields equal `keys`; exactly one must match.
fn by<'a>(rows: &'a [Value], keys: &[(&str, Value)]) -> &'a Value {
    let matches: Vec<&Value> = rows
        .iter()
        .filter(|row| keys.iter().all(|(name, value)| row[*name] == *value))
        .collect();
    assert_eq!(matches.len(), 1, "rows matching {keys:?}: {rows:?}");
    matches[0]
}

fn rows_of(report: &Value, field: &str) -> Vec<Value> {
    report[field].as_array().cloned().unwrap_or_default()
}

fn week_report(input: &AnalyticsInput) -> Value {
    subagents::report(input, &workspace_scope(), Period::Week, &Window::default())
}

fn month_report(input: &AnalyticsInput) -> Value {
    subagents::report(input, &workspace_scope(), Period::Month, &Window::default())
}

fn notes(report: &Value) -> Vec<String> {
    rows_of(report, "notes")
        .iter()
        .filter_map(|note| note.as_str().map(str::to_owned))
        .collect()
}

// covers: C014
#[test]
fn trial_runs_are_excluded_and_noted() {
    let report = week_report(&subagent_seed());
    let haiku_w42 = by(
        &rows_of(&report, "by_period"),
        &[("period", json!("2026-W42")), ("model", json!(HAIKU))],
    )
    .clone();
    assert_eq!(haiku_w42["runs"], json!(2));
    assert_eq!(haiku_w42["pending"], json!(0));
    assert!(
        notes(&report)
            .iter()
            .any(|note| note.contains("1 runs are experiment trials"))
    );
}

// covers: C014
#[test]
fn outcomes_and_rates_are_bucketed_by_week() {
    let report = week_report(&subagent_seed());
    let periods = rows_of(&report, "by_period");

    let haiku_w41 = by(
        &periods,
        &[("period", json!("2026-W41")), ("model", json!(HAIKU))],
    );
    assert_eq!(haiku_w41["runs"], json!(2));
    assert_eq!(haiku_w41["linked_runs"], json!(2));
    assert_eq!(haiku_w41["tasks"], json!(1));
    assert_eq!(haiku_w41["accepted"], json!(1));
    // acc1 has exactly one haiku run, so it is a first pass; acc2 moved to W42 on its latest run.
    assert_eq!(haiku_w41["first_pass"], json!(1));
    assert_eq!(haiku_w41["median_output_tokens"].as_f64(), Some(1500.0));

    let haiku_w42 = by(
        &periods,
        &[("period", json!("2026-W42")), ("model", json!(HAIKU))],
    );
    assert_eq!(haiku_w42["runs"], json!(2));
    assert_eq!(haiku_w42["linked_runs"], json!(1));
    assert_eq!(haiku_w42["tasks"], json!(1));
    assert_eq!(haiku_w42["accepted"], json!(1));
    assert_eq!(haiku_w42["first_pass"], json!(0));

    let haiku_w40 = by(
        &periods,
        &[("period", json!("2026-W40")), ("model", json!(HAIKU))],
    );
    assert_eq!(haiku_w40["failed"], json!(1));
    assert_eq!(haiku_w40["acceptance_rate"].as_f64(), Some(0.0));

    let sonnet_w42 = by(
        &periods,
        &[("period", json!("2026-W42")), ("model", json!(SONNET))],
    );
    assert_eq!(sonnet_w42["tasks"], json!(2));
    assert_eq!(sonnet_w42["rejected"], json!(1));
    assert_eq!(sonnet_w42["pending"], json!(1));
    assert_eq!(sonnet_w42["acceptance_rate"].as_f64(), Some(0.0));
    assert_eq!(sonnet_w42["median_turns"].as_f64(), Some(30.0));
    assert_eq!(sonnet_w42["tool_errors"], json!(2));
}

// covers: C014
#[test]
fn outcomes_are_bucketed_by_month() {
    let report = month_report(&subagent_seed());
    let periods = rows_of(&report, "by_period");

    let september = by(
        &periods,
        &[("period", json!("2026-09")), ("model", json!(HAIKU))],
    );
    assert_eq!(september["runs"], json!(1));
    assert_eq!(september["failed"], json!(1));
    let october = by(
        &periods,
        &[("period", json!("2026-10")), ("model", json!(HAIKU))],
    );
    assert_eq!(october["runs"], json!(4));
    assert_eq!(october["tasks"], json!(2));
    assert_eq!(october["accepted"], json!(2));
    assert_eq!(october["first_pass"], json!(1));
}

// covers: M61
#[test]
fn totals_carry_the_outcomes_and_the_wilson_interval() {
    let report = week_report(&subagent_seed());
    let totals = rows_of(&report, "totals");

    let haiku = by(&totals, &[("model", json!(HAIKU))]);
    assert_eq!(haiku["runs"], json!(5));
    assert_eq!(haiku["linked_runs"], json!(4));
    assert_eq!(haiku["tasks"], json!(3));
    assert_eq!(haiku["accepted"], json!(2));
    assert_eq!(haiku["rejected"], json!(0));
    assert_eq!(haiku["failed"], json!(1));
    assert_eq!(haiku["pending"], json!(0));
    assert_eq!(haiku["acceptance_rate"].as_f64(), Some(2.0 / 3.0));
    let expected = wilson(2, 3, Z_95).expect("a valid interval");
    assert_eq!(haiku["wilson"], json!([expected.0, expected.1]));
    assert_eq!(haiku["first_pass"], json!(1));
    assert_eq!(haiku["median_output_tokens"].as_f64(), Some(1500.0));
    assert_eq!(haiku["tool_errors"], json!(1));

    let sonnet = by(&totals, &[("model", json!(SONNET))]);
    let none = wilson(0, 1, Z_95).expect("a valid interval");
    assert_eq!(sonnet["wilson"], json!([none.0, none.1]));

    let unknown = by(&totals, &[("model", json!("unknown"))]);
    assert_eq!(unknown["runs"], json!(1));
    assert_eq!(unknown["accepted"], json!(1));
    assert_eq!(unknown["acceptance_rate"].as_f64(), Some(1.0));
}

// covers: M61
#[test]
fn size_bands_prefer_the_change_surface_and_leave_out_merges() {
    let report = week_report(&subagent_seed());
    let sizes = rows_of(&report, "by_size");
    let haiku_rows: Vec<Value> = sizes
        .iter()
        .filter(|row| row["model"] == json!(HAIKU))
        .cloned()
        .collect();
    let bands: Vec<Value> = haiku_rows.iter().map(|row| row["band"].clone()).collect();
    assert_eq!(bands, vec![json!(4), json!(6), Value::Null]);

    // Commits 10+3 and 5+2 lines; the merge commit is ignored.
    let band4 = by(&haiku_rows, &[("band", json!(4))]);
    assert_eq!(band4["tasks"], json!(1));
    assert_eq!(band4["accepted"], json!(1));
    assert_eq!(band4["median_lines"].as_f64(), Some(20.0));
    // The change surface (100+20) wins over the 1000-line commit.
    let band6 = by(&haiku_rows, &[("band", json!(6))]);
    assert_eq!(band6["accepted"], json!(1));
    assert_eq!(band6["median_lines"].as_f64(), Some(120.0));
    let unknown = by(&haiku_rows, &[("band", Value::Null)]);
    assert_eq!(unknown["tasks"], json!(1));
    assert_eq!(unknown["accepted"], json!(0));
    assert_eq!(unknown["decided"], json!(1));
    assert_eq!(unknown["median_lines"], Value::Null);

    let sonnet_rows: Vec<Value> = sizes
        .iter()
        .filter(|row| row["model"] == json!(SONNET))
        .cloned()
        .collect();
    assert_eq!(
        by(&sonnet_rows, &[("band", json!(2))])["acceptance_rate"].as_f64(),
        Some(0.0)
    );
    assert_eq!(
        by(&sonnet_rows, &[("band", Value::Null)])["decided"],
        json!(0)
    );
}

// covers: M61
#[test]
fn model_filter_and_window_apply() {
    let input = subagent_seed();
    let only_haiku = subagents::report(
        &input,
        &workspace_scope(),
        Period::Week,
        &Window {
            model: Some(HAIKU.to_owned()),
            ..Window::default()
        },
    );
    assert!(
        rows_of(&only_haiku, "by_period")
            .iter()
            .all(|row| row["model"] == json!(HAIKU))
    );
    assert_eq!(rows_of(&only_haiku, "totals").len(), 1);

    // since is inclusive and until exclusive: r7 (10-06), r2 (10-07) and r3 (10-12) are in; r2b is out.
    let windowed = subagents::report(
        &input,
        &workspace_scope(),
        Period::Week,
        &Window {
            since: Some(ts("2026-10-06T00:00:00Z")),
            until: Some(ts("2026-10-13T00:00:00Z")),
            model: None,
        },
    );
    let totals = rows_of(&windowed, "totals");
    let haiku = by(&totals, &[("model", json!(HAIKU))]);
    assert_eq!(haiku["runs"], json!(1));
    assert_eq!(haiku["tasks"], json!(1));
    assert_eq!(haiku["accepted"], json!(1));
    assert_eq!(by(&totals, &[("model", json!(SONNET))])["runs"], json!(1));
    assert!(
        windowed["window"]["since"]
            .as_str()
            .is_some_and(|since| since.starts_with("2026-10-06"))
    );
    assert_eq!(windowed["unlinked_runs"], json!(0));
}

// covers: C014
#[test]
fn unlinked_runs_and_notes_follow_the_workspace() {
    let input = subagent_seed();
    let report = week_report(&input);
    assert_eq!(report["unlinked_runs"], json!(1));
    let all = notes(&report);
    assert!(all.iter().any(|note| {
        note == "1 runs have no Task-ID link; start a subagent prompt with `Task-ID: <KEY>` to link it"
    }));
    assert!(all.iter().any(|note| note.contains("no stopped_at")));

    // Reported for another workspace: the default-workspace runs are not counted there.
    let elsewhere = subagents::report(
        &input,
        &Scope {
            workspace_id: "W2".to_owned(),
            default_workspace_id: Some("W1".to_owned()),
        },
        Period::Week,
        &Window::default(),
    );
    assert_eq!(elsewhere["unlinked_runs"], json!(1));
    assert_eq!(rows_of(&elsewhere, "totals"), Vec::<Value>::new());
    let elsewhere_notes = notes(&elsewhere);
    assert!(
        elsewhere_notes
            .iter()
            .any(|note| note.contains("other workspaces"))
    );
    assert!(
        elsewhere_notes
            .iter()
            .any(|note| note.contains("default workspace"))
    );
}

/// The time breakdown of a run, as `_time()` builds it in `test_agent_time_report.py`.
fn time_breakdown(
    span: f64,
    model: f64,
    waiting: f64,
    classes: &[(&str, f64, i64)],
    slowest: Vec<Value>,
) -> Value {
    let tool: f64 = classes.iter().map(|(_, seconds, _)| seconds).sum();
    let class_map: Map<String, Value> = classes
        .iter()
        .map(|(name, seconds, calls)| {
            (
                (*name).to_owned(),
                json!({"seconds": seconds, "calls": calls}),
            )
        })
        .collect();
    json!({
        "version": 1,
        "span_seconds": span,
        "tool_seconds": tool,
        "model_seconds": model,
        "waiting_seconds": waiting,
        "classes": class_map,
        "slowest": slowest,
    })
}

fn slow_call(class: &str, seconds: f64, command: &str, at: &str) -> Value {
    json!({"class": class, "seconds": seconds, "tool": "Bash", "command": command, "at": at})
}

fn timed_run(
    start: Timestamp,
    stop: Timestamp,
    model: &str,
    task: Option<&str>,
    telemetry: Value,
) -> SubagentRunRow {
    SubagentRunRow {
        task_id: task.map(str::to_owned),
        model: Some(model.to_owned()),
        started_at: Some(start),
        stopped_at: Some(stop),
        telemetry: Some(telemetry.to_string()),
        ..Default::default()
    }
}

/// `_seed` of `test_agent_time_report.py`: two overlapping linked runs, an unlinked run, a trial, a
/// run with no breakdown, and a run before the window.
fn time_seed() -> AnalyticsInput {
    let tasks: VectorSync<TaskRow> = [TaskRow {
        id: Some("timed".to_owned()),
        key: Some("WG-timed".to_owned()),
        workspace_id: Some("W1".to_owned()),
        status: Some("running".to_owned()),
        ..Default::default()
    }]
    .into_iter()
    .collect();
    let implementer = time_breakdown(
        600.0,
        100.0,
        0.0,
        &[("cargo test", 240.0, 2), ("search/read", 60.0, 5)],
        vec![slow_call(
            "cargo test",
            240.0,
            "cargo test -p x",
            "2026-10-14T10:01:00Z",
        )],
    );
    let poller = time_breakdown(
        360.0,
        0.0,
        300.0,
        &[("polling loop", 200.0, 1)],
        vec![slow_call(
            "polling loop",
            200.0,
            "until test -f x; do sleep 5; done",
            "2026-10-14T10:03:00Z",
        )],
    );
    let unlinked = time_breakdown(60.0, 50.0, 0.0, &[("search/read", 10.0, 1)], Vec::new());
    let trial = time_breakdown(30.0, 0.0, 0.0, &[("git", 30.0, 1)], Vec::new());
    let runs: VectorSync<SubagentRunRow> = [
        timed_run(
            hm(10, 0),
            hm(10, 10),
            HAIKU,
            Some("timed"),
            json!({"time": implementer, "description": "implementer"}),
        ),
        timed_run(
            hm(10, 2),
            hm(10, 8),
            HAIKU,
            Some("timed"),
            json!({"time": poller, "description": "reviewer"}),
        ),
        timed_run(
            hm(10, 0),
            hm(10, 1),
            SONNET,
            None,
            json!({"time": unlinked}),
        ),
        timed_run(
            hm(10, 0),
            hm(10, 0),
            HAIKU,
            None,
            json!({"time": trial, "trial_ids": ["trial-1"]}),
        ),
        timed_run(hm(9, 0), hm(9, 1), HAIKU, Some("timed"), json!({})),
    ]
    .into_iter()
    .collect();
    AnalyticsInput {
        tasks,
        subagent_runs: runs,
        ..Default::default()
    }
}

fn time_breakdown_report(input: &AnalyticsInput, by: Group, window: &Window) -> Value {
    agent_time::breakdown(input, &workspace_scope(), window, by)
}

// covers: C015
#[test]
fn breakdown_by_class_sums_tool_model_and_waiting() {
    let input = time_seed();
    let report = time_breakdown_report(&input, Group::Class, &Window::default());
    let rows = rows_of(&report, "rows");

    let class = |name: &str| by(&rows, &[("class", json!(name))]).clone();
    assert_eq!(class("cargo test")["seconds"].as_f64(), Some(240.0));
    assert_eq!(class("cargo test")["calls"], json!(2));
    assert_eq!(class("polling loop")["seconds"].as_f64(), Some(200.0));
    assert_eq!(
        class("waiting on background")["seconds"].as_f64(),
        Some(300.0)
    );
    assert_eq!(class("waiting on background")["kind"], json!("waiting"));
    assert_eq!(class("model")["seconds"].as_f64(), Some(150.0));
    assert_eq!(class("model")["kind"], json!("model"));
    assert!(
        rows.iter().all(|row| row["class"] != json!("git")),
        "the trial is excluded"
    );
    assert_eq!(report["totals"]["runs"], json!(3));
    assert_eq!(
        report["totals"]["accounted_seconds"].as_f64(),
        Some(240.0 + 60.0 + 200.0 + 10.0 + 150.0 + 300.0)
    );
    let share_sum: f64 = rows.iter().filter_map(|row| row["share"].as_f64()).sum();
    assert!((share_sum - 1.0).abs() < 1e-9);
    assert!(
        notes(&report)
            .iter()
            .any(|note| note.contains("no time breakdown"))
    );
}

// covers: C015
#[test]
fn breakdown_by_model_task_and_description() {
    let input = time_seed();
    let by_model = time_breakdown_report(&input, Group::Model, &Window::default());
    let keys: Vec<Value> = rows_of(&by_model, "rows")
        .iter()
        .map(|row| row["key"].clone())
        .collect();
    assert!(keys.contains(&json!(HAIKU)) && keys.contains(&json!(SONNET)));
    assert_eq!(keys.len(), 2);

    let by_task = time_breakdown_report(&input, Group::Task, &Window::default());
    let timed = by(&rows_of(&by_task, "rows"), &[("key", json!("WG-timed"))]).clone();
    assert_eq!(timed["runs"], json!(2));
    let unlinked = by(&rows_of(&by_task, "rows"), &[("key", json!("(unlinked)"))]).clone();
    assert_eq!(unlinked["runs"], json!(1));

    let by_description = time_breakdown_report(&input, Group::Description, &Window::default());
    let mut descriptions: Vec<String> = rows_of(&by_description, "rows")
        .iter()
        .filter_map(|row| row["key"].as_str().map(str::to_owned))
        .collect();
    descriptions.sort();
    assert_eq!(descriptions, vec!["implementer", "reviewer", "unknown"]);
}

// covers: C015, M62
#[test]
fn window_and_model_filters_apply() {
    let input = time_seed();
    let late = time_breakdown_report(
        &input,
        Group::Class,
        &Window {
            since: Some(hm(10, 9)),
            ..Window::default()
        },
    );
    // Only the implementer stopped after 10:09.
    assert_eq!(late["totals"]["runs"], json!(1));
    let haiku = time_breakdown_report(
        &input,
        Group::Class,
        &Window {
            model: Some(HAIKU.to_owned()),
            ..Window::default()
        },
    );
    assert_eq!(haiku["totals"]["runs"], json!(2));
}

// covers: C003, M63
#[test]
fn bottlenecks_report_concurrency_and_wall_time() {
    let input = time_seed();
    let report = agent_time::bottlenecks(&input, &workspace_scope(), &Window::default(), 5);
    let slowest = rows_of(&report, "slowest_calls");
    let classes: Vec<Value> = slowest.iter().map(|call| call["class"].clone()).collect();
    assert_eq!(classes, vec![json!("cargo test"), json!("polling loop")]);
    // At 10:01 the implementer (10:00-10:10) and the unlinked run (10:00-10:01) were live.
    assert_eq!(slowest[0]["concurrent_runs"], json!(2));
    // At 10:03 the implementer and the poller (10:02-10:08) were live.
    assert_eq!(slowest[1]["concurrent_runs"], json!(2));
    // At 10:00 the implementer, the unlinked run and the trial all started; concurrency is machine-wide.
    assert_eq!(report["concurrency"]["peak_during_window"], json!(3));

    let by_task = rows_of(&report, "by_task");
    let timed = by(&by_task, &[("key", json!("WG-timed"))]).clone();
    assert_eq!(timed["span_seconds"].as_f64(), Some(960.0));
    assert_eq!(timed["top_classes"][0]["class"], json!("cargo test"));
    assert_eq!(timed["top_classes"][0]["seconds"].as_f64(), Some(240.0));
    assert!(!timed["polling_share"].is_null());
    assert!(!report["totals"]["waiting_share"].is_null());
}
