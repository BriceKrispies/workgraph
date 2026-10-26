//! Automation-family reads (SH-12) against real SQLite: rules, runs (plain and joined to their
//! rule), the rate-limit count, notifications, reports and findings, with the filters, orderings
//! and pages the Python services use.
//!
//! The database is built from `contracts/schema-0011.sql`, as the SH-01, SH-04 and SH-05 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use wg_model::entity::{
    AutomationRule, AutomationRun, Finding, JsonObject, Notification, Real, Report, Workspace,
};
use wg_model::{Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_storecodec::automation_sql::Page;
use wg_types::{Id, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn schema() -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let wanted: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|block| {
            (block.starts_with("CREATE TABLE") && !block.starts_with("CREATE TABLE 'task_fts_"))
                || block.starts_with("CREATE VIRTUAL TABLE")
                || block.starts_with("CREATE TRIGGER")
        })
        .collect();
    Ok(wanted.join(";\n"))
}

fn store() -> Result<Store, Box<dyn Error>> {
    let mut store = Store::open(&Location::Memory)?;
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(store)
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(12), n)
}

/// A timestamp on the fixture day, `minute` minutes past noon.
fn at(minute: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, minute, 0, 0))
}

/// Inserts every row in order, one write each, through the generic write path.
fn insert_all(store: &mut Store, rows: Vec<Row>) -> TestResult {
    store.write(|session| {
        for row in rows {
            session.apply_write(&Write::Insert(row))?;
        }
        Ok(())
    })?;
    Ok(())
}

/// A `trigger` object with the given `type`.
fn trigger(kind: &str) -> JsonObject {
    let mut object = JsonObject::new();
    object.insert("type".into(), kind.into());
    object
}

fn workspace(n: u32, slug: &str) -> Workspace {
    Workspace {
        id: id(n),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 1,
        settings: Default::default(),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn rule(n: u32, workspace: u32, name: &str, kind: &str, enabled: bool) -> AutomationRule {
    AutomationRule {
        id: id(n),
        workspace_id: id(workspace),
        name: name.into(),
        description: String::new(),
        enabled,
        trigger: trigger(kind),
        conditions: JsonObject::new(),
        actions: VectorSync::new_sync(),
        retry_max_attempts: 3,
        max_depth: 3,
        last_scheduled_at: None,
        next_run_at: None,
        created_by: "system".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
    }
}

fn run(
    n: u32,
    rule: u32,
    workspace: u32,
    key: &str,
    status: &str,
    dry: bool,
    minute: u8,
) -> AutomationRun {
    AutomationRun {
        id: id(n),
        rule_id: id(rule),
        workspace_id: id(workspace),
        trigger_type: "event".into(),
        trigger_key: key.into(),
        trigger_event_id: None,
        depth: 0,
        dry_run: dry,
        status: status.into(),
        matched: true,
        reason: String::new(),
        condition_trace: VectorSync::new_sync(),
        action_results: VectorSync::new_sync(),
        error: None,
        attempts: 1,
        correlation_id: None,
        created_at: at(minute),
        finished_at: None,
        inputs: JsonObject::new(),
    }
}

fn notification(n: u32, workspace: u32, read: bool, minute: u8) -> Notification {
    Notification {
        id: id(n),
        workspace_id: id(workspace),
        level: "info".into(),
        title: format!("note {n}"),
        body: String::new(),
        source: "automation".into(),
        data: JsonObject::new(),
        read_at: read.then(|| at(59)),
        created_at: at(minute),
    }
}

fn report(n: u32, workspace: u32, minute: u8) -> Report {
    Report {
        id: id(n),
        workspace_id: id(workspace),
        kind: "retrospective".into(),
        title: format!("report {n}"),
        scope: JsonObject::new(),
        content_markdown: String::new(),
        data: JsonObject::new(),
        narrative_source: "deterministic".into(),
        created_by: "system".into(),
        created_at: at(minute),
    }
}

fn finding(n: u32, workspace: u32, fingerprint: &str) -> Finding {
    Finding {
        id: id(n),
        workspace_id: id(workspace),
        kind: "slow_task".into(),
        fingerprint: fingerprint.into(),
        title: format!("finding {n}"),
        severity: "warning".into(),
        metric_value: Some(Real(0.5)),
        sample_size: 4,
        evidence: JsonObject::new(),
        first_detected_at: at(1),
        last_detected_at: at(2),
    }
}

/// Two workspaces. Workspace 1 has three rules (`nightly` schedule, `on-done` event, `paused` event
/// and disabled); workspace 2 has an event rule that shares the name `nightly`. Rule `on-done` has
/// five runs, rule `other` has one run whose key `evt:1` also belongs to `on-done`.
fn seeded() -> Result<Store, Box<dyn Error>> {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            workspace(1, "default").into(),
            workspace(2, "other").into(),
            rule(10, 1, "nightly", "schedule", true).into(),
            rule(11, 1, "on-done", "event", true).into(),
            rule(12, 1, "paused", "event", false).into(),
            rule(13, 2, "nightly", "event", true).into(),
            // (id, rule, workspace, key, status, dry, created minute)
            run(20, 11, 1, "evt:1", "succeeded", false, 10).into(),
            run(21, 11, 1, "evt:2", "failed", false, 20).into(),
            run(22, 11, 1, "evt:3", "queued", true, 30).into(),
            run(23, 11, 1, "evt:4", "skipped", false, 40).into(),
            run(24, 11, 1, "evt:5", "running", false, 50).into(),
            run(30, 13, 2, "evt:1", "succeeded", false, 15).into(),
        ],
    )?;
    Ok(store)
}

fn ids<T>(items: &[T], key: impl Fn(&T) -> Id) -> Vec<Id> {
    items.iter().map(key).collect()
}

#[test]
fn rule_lookup_is_per_workspace() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let first = session.automation_rule_by_name(id(1), "nightly")?;
        assert_eq!(first.map(|r| r.id), Some(id(10)));
        let second = session.automation_rule_by_name(id(2), "nightly")?;
        assert_eq!(second.map(|r| r.id), Some(id(13)));
        assert!(session.automation_rule_by_name(id(1), "missing")?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn rules_are_listed_by_name() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let rules = session.automation_rules(id(1))?;
        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["nightly", "on-done", "paused"]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn enabled_rules_are_filtered_by_trigger_type() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        // Disabled rules are left out. Both enabled event rules are returned, one per workspace;
        // the order between workspaces follows the workspace id, so compare as sets.
        let event = session.enabled_automation_rules("event")?;
        let mut event_ids = ids(&event, |r| r.id);
        event_ids.sort();
        let mut expected = vec![id(11), id(13)];
        expected.sort();
        assert_eq!(event_ids, expected);
        let schedule = session.enabled_automation_rules("schedule")?;
        assert_eq!(ids(&schedule, |r| r.id), vec![id(10)]);
        assert!(session.enabled_automation_rules("manual")?.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn trigger_key_lookups() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let hit = session.automation_run_by_trigger(id(11), "evt:2")?;
        assert_eq!(hit.map(|r| r.id), Some(id(21)));
        // The same key under another rule is not this rule's run.
        assert!(
            session
                .automation_run_by_trigger(id(13), "evt:2")?
                .is_none()
        );
        // Across rules the oldest run with the key is the one returned (run 20, not run 30).
        let oldest = session.automation_run_by_trigger_key("evt:1")?;
        assert_eq!(oldest.map(|r| r.id), Some(id(20)));
        assert!(session.automation_run_by_trigger_key("evt:9")?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn runs_are_newest_first_and_paged() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let first = session.automation_runs(id(1), None, None, Page::first(2))?;
        assert_eq!(ids(&first, |r| r.id), vec![id(24), id(23)]);
        let middle = session.automation_runs(
            id(1),
            None,
            None,
            Page {
                limit: 2,
                offset: 2,
            },
        )?;
        assert_eq!(ids(&middle, |r| r.id), vec![id(22), id(21)]);
        let last = session.automation_runs(
            id(1),
            None,
            None,
            Page {
                limit: 2,
                offset: 4,
            },
        )?;
        assert_eq!(ids(&last, |r| r.id), vec![id(20)]);
        // Workspace 2 sees only its own run.
        let other = session.automation_runs(id(2), None, None, Page::first(10))?;
        assert_eq!(ids(&other, |r| r.id), vec![id(30)]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn runs_narrow_by_rule_and_status() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let failed =
            session.automation_runs(id(1), Some(id(11)), Some("failed"), Page::first(10))?;
        assert_eq!(ids(&failed, |r| r.id), vec![id(21)]);
        let none = session.automation_runs(id(1), Some(id(10)), None, Page::first(10))?;
        assert!(none.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn runs_joined_to_their_rule() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        let rows = session.automation_runs_with_rule(id(1), Some(id(11)), None, Page::first(1))?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run.id, id(24));
        assert_eq!(rows[0].run.trigger_key, "evt:5");
        assert_eq!(rows[0].rule_name, "on-done");

        let all = session.automation_runs_with_rule(id(1), None, None, Page::first(10))?;
        assert_eq!(all.len(), 5);
        assert!(all.iter().all(|row| row.rule_name == "on-done"));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn live_run_count_follows_the_rate_limit_rule() -> TestResult {
    let mut store = seeded()?;
    store.read(|session| {
        // Live and not dry: runs 20, 21 and 24. Run 22 is a dry run and run 23 is `skipped`.
        assert_eq!(session.automation_live_run_count(id(11), at(0))?, 3);
        // Only runs created strictly after the window start: 21 (minute 20) and 24 (minute 50).
        assert_eq!(session.automation_live_run_count(id(11), at(15))?, 2);
        assert_eq!(session.automation_live_run_count(id(10), at(0))?, 0);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn notifications_filter_and_page() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            workspace(1, "default").into(),
            notification(40, 1, false, 1).into(),
            notification(41, 1, true, 2).into(),
            notification(42, 1, false, 3).into(),
        ],
    )?;
    store.read(|session| {
        let all = session.notifications(id(1), false, Page::first(10))?;
        assert_eq!(ids(&all, |n| n.id), vec![id(42), id(41), id(40)]);
        let unread = session.notifications(id(1), true, Page::first(10))?;
        assert_eq!(ids(&unread, |n| n.id), vec![id(42), id(40)]);
        let second_unread = session.notifications(
            id(1),
            true,
            Page {
                limit: 1,
                offset: 1,
            },
        )?;
        assert_eq!(ids(&second_unread, |n| n.id), vec![id(40)]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn reports_are_listed_newest_first() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            workspace(1, "default").into(),
            workspace(2, "other").into(),
            report(50, 1, 1).into(),
            report(51, 1, 2).into(),
            report(52, 2, 3).into(),
        ],
    )?;
    store.read(|session| {
        let listed = session.reports(id(1), Page::first(20))?;
        assert_eq!(ids(&listed, |r| r.id), vec![id(51), id(50)]);
        let latest = session.reports(id(1), Page::first(1))?;
        assert_eq!(ids(&latest, |r| r.id), vec![id(51)]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn findings_are_found_by_fingerprint_per_workspace() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            workspace(1, "default").into(),
            workspace(2, "other").into(),
            finding(60, 1, "abc").into(),
        ],
    )?;
    store.read(|session| {
        let hit = session.finding_by_fingerprint(id(1), "abc")?;
        assert_eq!(hit.map(|f| f.id), Some(id(60)));
        assert!(session.finding_by_fingerprint(id(2), "abc")?.is_none());
        assert!(session.finding_by_fingerprint(id(1), "zzz")?.is_none());
        Ok(())
    })?;
    // The fingerprint is unique within a workspace: the upsert in failures.py relies on it.
    let duplicate =
        store.write(|session| session.apply_write(&Write::Insert(finding(61, 1, "abc").into())));
    assert!(duplicate.is_err());
    Ok(())
}
