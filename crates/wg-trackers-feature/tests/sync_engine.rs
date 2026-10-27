//! The tracker sync engine (`TrackerSyncService.sync`) against a real database and a fake Jira
//! transport: the listing and the references are served from memory, and every write goes through
//! the store as `system:tracker-sync`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wg_model::Decision;
use wg_model::Write;
use wg_model::entity::{JsonObject, Workspace};
use wg_netspec::TrackerError;
use wg_runtime::Queue;
use wg_store::{EventQuery, TaskWorld};
use wg_tasks::edit::Enrichment;
use wg_trackers::SyncReport;
use wg_trackers_feature::service::sync::{IssueSource, sync};
use wg_trackers_feature::spec::run::settings_of;
use wg_types::{Correlation, Id, TaskKey, TaskStatus, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

/// A transport that serves the listing, the issues and the key list from memory. Clones share the
/// record of the `since` values the listing was asked for.
#[derive(Clone, Default)]
struct FakeJira {
    listing: Vec<Value>,
    keys: Vec<(String, String)>,
    issues: HashMap<String, Value>,
    since_seen: Arc<Mutex<Vec<Option<i64>>>>,
}

impl IssueSource for FakeJira {
    fn search_updated(
        &mut self,
        _projects: &[String],
        since: Option<i64>,
    ) -> Result<Vec<Value>, TrackerError> {
        self.since_seen.lock().unwrap().push(since);
        Ok(self.listing.clone())
    }

    fn list_keys(&mut self, _projects: &[String]) -> Result<Vec<(String, String)>, TrackerError> {
        Ok(self.keys.clone())
    }

    fn get_issue(&mut self, key: &str) -> Result<Option<Value>, TrackerError> {
        Ok(self.issues.get(key).cloned())
    }

    fn dev_pull_requests(&mut self, _external_id: &str) -> Vec<Value> {
        Vec::new()
    }
}

/// One Jira issue in the shape the mapper reads.
fn issue(
    id: &str,
    key: &str,
    summary: &str,
    status: (&str, &str),
    resolution: Option<&str>,
    parent: Option<&str>,
    updated: &str,
) -> Value {
    json!({
        "id": id,
        "key": key,
        "fields": {
            "summary": summary,
            "description": null,
            "priority": {"name": "Medium"},
            "assignee": null,
            "status": {"name": status.0, "statusCategory": {"key": status.1}},
            "resolution": resolution.map(|name| json!({"name": name})),
            "issuetype": {"name": "Task"},
            "project": {"key": "WG"},
            "parent": parent.map(|key| json!({"key": key})),
            "issuelinks": [],
            "labels": [],
            "updated": updated
        }
    })
}

/// A fresh Workgraph home with one tracker connection, `jira1`, on the `WG` project.
fn home(name: &str) -> Result<(PathBuf, Queue), Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sync-engine")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let queue = Queue::open(Some(&dir), "test")?;
    std::fs::write(
        queue.app.paths.config_file().as_str(),
        "[trackers.jira1]\nbase_url = \"https://jira.example\"\nworkspace = \"default\"\nprojects = [\"WG\"]\ntoken_env = \"JIRA_TOKEN\"\noverlap_seconds = 0\n",
    )?;
    Ok((dir, queue))
}

/// The `default` workspace the connection mirrors into.
fn workspace(queue: &mut Queue) -> Result<Id, Box<dyn Error>> {
    let id = queue.app.runtime.store.write(|session| {
        let at = Timestamp::from_components(2026, 10, 26, 9, 0, 0, 0)?;
        let id = wg_types::parse_id("00000000-0000-4000-8000-000000000001")?;
        let row = Workspace {
            id,
            slug: "default".to_owned(),
            name: "Default".to_owned(),
            description: String::new(),
            key_prefix: "LOCAL".to_owned(),
            next_task_number: 1,
            settings: JsonObject::new(),
            archived_at: None,
            created_at: at,
            updated_at: at,
        };
        session.apply_decision(&Decision::new(()).write(Write::insert(row)))?;
        Ok(id)
    })?;
    Ok(id)
}

/// One sync of `jira1` with the fake transport, as `tracker sync` runs it.
fn sync_now(queue: &mut Queue, fake: &FakeJira, full: bool) -> Result<SyncReport, Box<dyn Error>> {
    let config = queue.app.config()?;
    let settings = settings_of(&config, "jira1")?;
    let report = sync(
        &mut queue.app,
        || Ok(fake.clone()),
        Correlation::new("sync-test"),
        &settings,
        Enrichment::default(),
        full,
    )?;
    Ok(report)
}

/// The workspace's task world and event types, read back from the store.
fn read(queue: &mut Queue, ws: Id) -> Result<(TaskWorld, Vec<String>), Box<dyn Error>> {
    let read = queue.app.runtime.store.read(|session| {
        let world = session.task_world(Some(ws), &[])?;
        let events = session.query_events(&EventQuery {
            workspace_id: Some(ws),
            ..EventQuery::default()
        })?;
        Ok((
            world,
            events.into_iter().map(|event| event.event_type).collect(),
        ))
    })?;
    Ok(read)
}

fn status_of(world: &TaskWorld, ws: Id, key: &str) -> Option<TaskStatus> {
    let parsed = TaskKey::parse(key).ok()?;
    world.tasks.task_by_key(ws, &parsed).map(|task| task.status)
}

fn count(events: &[String], event_type: &str) -> usize {
    events
        .iter()
        .filter(|event| event.as_str() == event_type)
        .count()
}

// covers: C170
#[test]
fn a_first_sync_mirrors_the_issues_with_their_parent_project_and_cursor() -> TestResult {
    let (_dir, mut queue) = home("first")?;
    let ws = workspace(&mut queue)?;
    let fake = FakeJira {
        listing: vec![
            issue(
                "1001",
                "WG-2",
                "Child",
                ("In Progress", "indeterminate"),
                None,
                Some("WG-1"),
                "2026-10-20T10:00:00.000+0000",
            ),
            issue(
                "1000",
                "WG-1",
                "Parent",
                ("To Do", "new"),
                None,
                None,
                "2026-10-19T10:00:00.000+0000",
            ),
        ],
        ..FakeJira::default()
    };
    let report = sync_now(&mut queue, &fake, false)?;

    assert_eq!(report.fetched, 2);
    assert_eq!(report.created, 2);
    assert_eq!(report.updated, 0);
    assert_eq!(report.unchanged, 0);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(*fake.since_seen.lock().unwrap(), vec![None]);
    assert_eq!(
        report.cursor,
        Some(1_792_490_400_000),
        "the newest remote update, in epoch milliseconds"
    );

    let (world, events) = read(&mut queue, ws)?;
    let child = world
        .tasks
        .task_by_key(ws, &TaskKey::parse("WG-2")?)
        .ok_or("WG-2 missing")?;
    let parent = world
        .tasks
        .task_by_key(ws, &TaskKey::parse("WG-1")?)
        .ok_or("WG-1 missing")?;
    assert_eq!(child.parent_id, Some(parent.id));
    assert_eq!(child.created_by, "system:tracker-sync");
    assert_eq!(world.tasks.external_items.size(), 2);
    assert_eq!(world.tasks.registry.projects.size(), 1);
    assert_eq!(count(&events, "task.created"), 2);
    assert_eq!(count(&events, "task.synced"), 2);
    assert_eq!(count(&events, "project.created"), 1);
    Ok(())
}

// covers: C170
#[test]
fn a_second_sync_uses_the_cursor_and_changes_nothing() -> TestResult {
    let (_dir, mut queue) = home("second")?;
    let _ws = workspace(&mut queue)?;
    let listing = vec![issue(
        "1000",
        "WG-1",
        "Parent",
        ("To Do", "new"),
        None,
        None,
        "2026-10-19T10:00:00.000+0000",
    )];
    let first_fake = FakeJira {
        listing: listing.clone(),
        ..FakeJira::default()
    };
    let first = sync_now(&mut queue, &first_fake, false)?;
    assert_eq!(first.created, 1);

    let again = FakeJira {
        listing,
        ..FakeJira::default()
    };
    let second = sync_now(&mut queue, &again, false)?;
    assert_eq!(second.created, 0);
    assert_eq!(second.updated, 0);
    assert_eq!(second.unchanged, 1);
    // The overlap is zero in this connection, so the listing starts at the cursor's second.
    let expected_since = first.cursor.map(|ms| ms.div_euclid(1000));
    assert_eq!(*again.since_seen.lock().unwrap(), vec![expected_since]);
    Ok(())
}

// covers: C170
#[test]
fn a_changed_title_updates_the_task_and_a_full_sync_marks_what_it_no_longer_lists() -> TestResult {
    let (_dir, mut queue) = home("changes")?;
    let ws = workspace(&mut queue)?;
    let first = FakeJira {
        listing: vec![
            issue(
                "1000",
                "WG-1",
                "Parent",
                ("To Do", "new"),
                None,
                None,
                "2026-10-19T10:00:00.000+0000",
            ),
            issue(
                "1001",
                "WG-2",
                "Other",
                ("To Do", "new"),
                None,
                None,
                "2026-10-19T11:00:00.000+0000",
            ),
        ],
        ..FakeJira::default()
    };
    sync_now(&mut queue, &first, false)?;

    let renamed = FakeJira {
        listing: vec![issue(
            "1000",
            "WG-1",
            "Parent renamed",
            ("To Do", "new"),
            None,
            None,
            "2026-10-21T10:00:00.000+0000",
        )],
        keys: vec![("1000".to_owned(), "WG-1".to_owned())],
        ..FakeJira::default()
    };
    let report = sync_now(&mut queue, &renamed, true)?;
    assert_eq!(report.updated, 1);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.missing, 1, "WG-2 is no longer listed");
    assert_eq!(report.archived, 0, "the grace period has not passed");

    let (world, events) = read(&mut queue, ws)?;
    let parent = world
        .tasks
        .task_by_key(ws, &TaskKey::parse("WG-1")?)
        .ok_or("WG-1 missing")?;
    assert_eq!(parent.title, "Parent renamed");
    assert!(count(&events, "task.updated") >= 1);
    let other = world
        .tasks
        .external_items
        .values()
        .find(|item| item.external_id == "1001")
        .ok_or("item missing")?;
    assert!(other.missing_since.is_some());
    Ok(())
}

// covers: C170
#[test]
fn a_cancelled_resolution_closes_the_task_and_a_close_as_done_accepts_it_externally() -> TestResult
{
    let (_dir, mut queue) = home("status")?;
    let ws = workspace(&mut queue)?;
    let fake = FakeJira {
        listing: vec![
            issue(
                "1000",
                "WG-1",
                "Dropped",
                ("Done", "done"),
                Some("Won't Do"),
                None,
                "2026-10-19T10:00:00.000+0000",
            ),
            issue(
                "1001",
                "WG-2",
                "Finished",
                ("Done", "done"),
                Some("Done"),
                None,
                "2026-10-19T11:00:00.000+0000",
            ),
        ],
        ..FakeJira::default()
    };
    let report = sync_now(&mut queue, &fake, false)?;
    assert_eq!(report.created, 2);
    let (world, events) = read(&mut queue, ws)?;
    assert_eq!(status_of(&world, ws, "WG-1"), Some(TaskStatus::Cancelled));
    assert_eq!(status_of(&world, ws, "WG-2"), Some(TaskStatus::Accepted));
    assert!(count(&events, "task.status_changed") >= 2);
    // The done issue records its external acceptance, as `record_external_acceptance` does.
    assert_eq!(count(&events, "task.accepted"), 1);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    Ok(())
}

// covers: C170
#[test]
fn a_referenced_parent_outside_the_listing_is_fetched_but_not_counted_as_listed() -> TestResult {
    let (_dir, mut queue) = home("fetch")?;
    let ws = workspace(&mut queue)?;
    let fake = FakeJira {
        listing: vec![issue(
            "1001",
            "WG-2",
            "Child",
            ("To Do", "new"),
            None,
            Some("WG-9"),
            "2026-10-19T11:00:00.000+0000",
        )],
        issues: [(
            "WG-9".to_owned(),
            issue(
                "1009",
                "WG-9",
                "Remote parent",
                ("To Do", "new"),
                None,
                None,
                "2026-10-18T09:00:00.000+0000",
            ),
        )]
        .into_iter()
        .collect(),
        ..FakeJira::default()
    };
    let report = sync_now(&mut queue, &fake, false)?;
    assert_eq!(report.fetched, 1, "only the listing counts as fetched");
    assert_eq!(report.created, 2);
    let (world, _events) = read(&mut queue, ws)?;
    let child = world
        .tasks
        .task_by_key(ws, &TaskKey::parse("WG-2")?)
        .ok_or("WG-2 missing")?;
    let parent = world
        .tasks
        .task_by_key(ws, &TaskKey::parse("WG-9")?)
        .ok_or("WG-9 missing")?;
    assert_eq!(child.parent_id, Some(parent.id));
    Ok(())
}
