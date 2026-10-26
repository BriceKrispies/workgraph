//! CORE-23: the sync decisions of `src/workgraph/integrations/trackers/sync.py`, as pure functions.
//!
//! The expected values for `snapshot_hash`, `dependency_order` and `field_updates` were produced
//! by the Python functions themselves (`uv run python`, calling `_snapshot_hash`, `_ordered` and
//! `_field_updates` on the same inputs). The dependency plan mirrors `_sync_dependencies`, whose
//! database reads are the shell's job, so its cases follow that function's branches directly.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;
use wg_trackers::{
    BlockerTask, ExistingDependency, MappingConfig, ParentFields, PullRequestRef, StatusCategory,
    SyncLinks, TaskFields, TrackerSnapshot, dependency_order, dependency_plan, field_updates,
    map_issue, project_of, snapshot_hash,
};
use wg_types::{TaskPriority, Timestamp};

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/fixtures/jira/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {path}: {err}"));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parsing {path}: {err}"))
}

fn snap(key: &str, parent: Option<&str>, blockers: &[&str]) -> TrackerSnapshot {
    TrackerSnapshot {
        external_id: key.to_string(),
        key: key.to_string(),
        url: "u".to_string(),
        project_key: "P".to_string(),
        title: "T".to_string(),
        description: "D".to_string(),
        priority: TaskPriority::Medium,
        assignee: None,
        status_name: String::new(),
        status_category: StatusCategory::New,
        resolution: None,
        parent_key: parent.map(str::to_string),
        blocked_by_keys: blockers.iter().map(|key| key.to_string()).collect(),
        pull_requests: Vec::new(),
        remote_updated_at: Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("timestamp"),
        issue_type: String::new(),
        labels: Vec::new(),
        raw: Value::Null,
    }
}

fn keys(snaps: &[&TrackerSnapshot]) -> Vec<String> {
    snaps.iter().map(|snap| snap.key.clone()).collect()
}

// --- snapshot_hash ---------------------------------------------------------------------------------

#[test]
fn snapshot_hash_matches_the_python_digest() {
    let config = MappingConfig {
        base_url: "https://acme.atlassian.net".to_string(),
        pr_fields: vec!["customfield_10042".to_string()],
        dev_panel: true,
    };
    let dev = fixture("dev_status.json")["detail"][0]["pullRequests"]
        .as_array()
        .cloned()
        .expect("dev pull requests");
    let story = map_issue(&fixture("cloud_story.json"), &dev, &config).expect("maps");

    // Python: _snapshot_hash(story, {"repositories": ["r-é☃"], "parent": None, "blockers": []})
    let first = SyncLinks {
        repositories: vec!["r-\u{e9}\u{2603}".to_string()],
        parent: None,
        blockers: Vec::new(),
    };
    assert_eq!(
        snapshot_hash(&story, &first),
        "cff94bc32fdee192127ce0c4c813fd61a542879c28384798bfc7e5047cb9afd3"
    );

    // Python: _snapshot_hash(story, {"repositories": [], "parent": "p", "blockers": ["b1", "b2"]})
    let second = SyncLinks {
        repositories: Vec::new(),
        parent: Some("p".to_string()),
        blockers: vec!["b1".to_string(), "b2".to_string()],
    };
    assert_eq!(
        snapshot_hash(&story, &second),
        "2f973c637dfeb9d837ba0d33d3e15d7b9dc5486986600be29c57d38342d1566e"
    );
}

#[test]
fn snapshot_hash_changes_with_links_and_with_pull_requests() {
    let base = snap("ABC-1", None, &[]);
    let links = SyncLinks::default();
    let mut with_pr = base.clone();
    with_pr.pull_requests.push(PullRequestRef {
        url: "https://github.com/a/b/pull/1".to_string(),
        normalized_url: "github.com/a/b/pull/1".to_string(),
        provider: "github",
        repo_full_name: "a/b".to_string(),
        repo_normalized_url: "github.com/a/b".to_string(),
        number: 1,
        sources: vec!["custom_field"],
        title: String::new(),
    });
    assert_ne!(
        snapshot_hash(&base, &links),
        snapshot_hash(&with_pr, &links)
    );
    let linked = SyncLinks {
        parent: Some("p".to_string()),
        ..SyncLinks::default()
    };
    assert_ne!(snapshot_hash(&base, &links), snapshot_hash(&base, &linked));
}

// --- dependency_order -----------------------------------------------------------------------------

#[test]
fn dependency_order_puts_parents_and_blockers_first() {
    // Python: _ordered([C(parent B), B(blocked A), A]) -> A, B, C
    let snaps = [
        snap("C", Some("B"), &[]),
        snap("B", None, &["A"]),
        snap("A", None, &[]),
    ];
    assert_eq!(keys(&dependency_order(&snaps)), ["A", "B", "C"]);
}

#[test]
fn dependency_order_keeps_input_order_for_cycles() {
    // Python: [X(blocked Y), Z, Y(blocked X)] -> Y, X, Z
    let snaps = [
        snap("X", None, &["Y"]),
        snap("Z", None, &[]),
        snap("Y", None, &["X"]),
    ];
    assert_eq!(keys(&dependency_order(&snaps)), ["Y", "X", "Z"]);
}

#[test]
fn dependency_order_handles_self_references() {
    // Python: [X(parent X), W(blocked X, Q)] -> X, W
    let snaps = [snap("X", Some("X"), &[]), snap("W", None, &["X", "Q"])];
    assert_eq!(keys(&dependency_order(&snaps)), ["X", "W"]);
}

#[test]
fn dependency_order_resolves_duplicate_keys_to_the_later_snapshot() {
    // Python: [A(blocked B), B, A(no blockers)] -> B, A
    let snaps = [
        snap("A", None, &["B"]),
        snap("B", None, &[]),
        snap("A", None, &[]),
    ];
    let ordered = dependency_order(&snaps);
    assert_eq!(keys(&ordered), ["B", "A"]);
    // Python appends the first A object (the one visited first), not the later duplicate.
    assert_eq!(ordered[1].blocked_by_keys, ["B"]);
}

#[test]
fn dependency_order_ignores_references_outside_the_batch() {
    // Python: [A(parent NOPE, blocked ALSO), B] -> A, B
    let snaps = [snap("A", Some("NOPE"), &["ALSO"]), snap("B", None, &[])];
    assert_eq!(keys(&dependency_order(&snaps)), ["A", "B"]);
}

#[test]
fn dependency_order_visits_deep_chains() {
    // Python: [D(blocked C, B), B(parent A), C(blocked A), A] -> A, C, B, D
    let snaps = [
        snap("D", None, &["C", "B"]),
        snap("B", Some("A"), &[]),
        snap("C", None, &["A"]),
        snap("A", None, &[]),
    ];
    assert_eq!(keys(&dependency_order(&snaps)), ["A", "C", "B", "D"]);
}

#[test]
fn project_of_is_everything_before_the_last_hyphen() {
    assert_eq!(project_of("ABC-123"), "ABC");
    assert_eq!(project_of("MY-PROJ-7"), "MY-PROJ");
    assert_eq!(project_of("NOHYPHEN"), "NOHYPHEN");
}

// --- field_updates ---------------------------------------------------------------------------------

fn task<'a>() -> TaskFields<'a> {
    TaskFields {
        id: "T1",
        key: "ABC-1",
        title: "T",
        description: "D",
        priority: TaskPriority::Medium,
        assignee: None,
        parent_id: None,
    }
}

fn parent<'a>(id: &'a str, key: &'a str) -> Option<ParentFields<'a>> {
    Some(ParentFields { id, key })
}

#[test]
fn field_updates_is_empty_when_everything_matches() {
    let updates = field_updates(&task(), &snap("ABC-1", None, &[]), None, || false);
    assert_eq!(updates, Default::default());
    assert!(!updates.has_changes());
}

#[test]
fn field_updates_reports_each_differing_field() {
    // Python: {'assignee': 'Bob', 'description': 'D2', 'priority': 'high', 'title': 'T2'}
    let mut incoming = snap("ABC-1", None, &[]);
    incoming.title = "T2".to_string();
    incoming.description = "D2".to_string();
    incoming.priority = TaskPriority::High;
    incoming.assignee = Some("Bob".to_string());
    let updates = field_updates(&task(), &incoming, None, || false);
    assert_eq!(updates.title.as_deref(), Some("T2"));
    assert_eq!(updates.description.as_deref(), Some("D2"));
    assert_eq!(updates.priority, Some(TaskPriority::High));
    assert_eq!(updates.assignee, Some(Some("Bob".to_string())));
    assert!(updates.errors.is_empty());
}

#[test]
fn field_updates_clears_a_removed_assignee() {
    // Python: {'assignee': None}
    let mut current = task();
    current.assignee = Some("Ann");
    let updates = field_updates(&current, &snap("ABC-1", None, &[]), None, || false);
    assert_eq!(updates.assignee, Some(None));
}

#[test]
fn field_updates_clears_a_parent_the_snapshot_no_longer_names() {
    // Python: {'clear_parent': True}
    let mut current = task();
    current.parent_id = Some("P0");
    let updates = field_updates(&current, &snap("ABC-1", None, &[]), None, || false);
    assert!(updates.clear_parent);
    assert!(updates.parent_key.is_none());
}

#[test]
fn field_updates_leaves_the_parent_alone_when_it_is_unresolved() {
    // Python: {} for snapshot parent ABC-9 that is not a local task
    let mut current = task();
    current.parent_id = Some("P0");
    let updates = field_updates(&current, &snap("ABC-1", Some("ABC-9"), &[]), None, || false);
    assert!(!updates.has_changes());
}

#[test]
fn field_updates_sets_a_new_parent_by_key() {
    // Python: {'parent': 'ABC-9'}
    let updates = field_updates(
        &task(),
        &snap("ABC-1", Some("ABC-9"), &[]),
        parent("P9", "ABC-9"),
        // Python asks the ancestry question for any differing parent; the answer is no here.
        || false,
    );
    assert_eq!(updates.parent_key.as_deref(), Some("ABC-9"));
    assert!(updates.errors.is_empty());
}

#[test]
fn field_updates_keeps_a_matching_parent() {
    let mut current = task();
    current.parent_id = Some("P9");
    let updates = field_updates(
        &current,
        &snap("ABC-1", Some("ABC-9"), &[]),
        parent("P9", "ABC-9"),
        || panic!("matching parent needs no ancestry check"),
    );
    assert!(!updates.has_changes());
}

#[test]
fn field_updates_refuses_a_parent_that_is_an_ancestor() {
    // Python: error "ABC-1: parent ABC-9 would create a hierarchy cycle", no parent update
    let updates = field_updates(
        &task(),
        &snap("ABC-1", Some("ABC-9"), &[]),
        parent("P9", "ABC-9"),
        || true,
    );
    assert_eq!(
        updates.errors,
        ["ABC-1: parent ABC-9 would create a hierarchy cycle"]
    );
    assert!(updates.parent_key.is_none());
}

#[test]
fn field_updates_refuses_a_task_as_its_own_parent_without_asking() {
    // Python: error "ABC-1: parent ABC-1 would create a hierarchy cycle"
    let updates = field_updates(
        &task(),
        &snap("ABC-1", Some("ABC-1"), &[]),
        parent("T1", "ABC-1"),
        || panic!("self parent is refused before the ancestry check"),
    );
    assert_eq!(
        updates.errors,
        ["ABC-1: parent ABC-1 would create a hierarchy cycle"]
    );
    assert!(!updates.has_changes());
}

// --- dependency_plan -------------------------------------------------------------------------------

#[test]
fn dependency_plan_adds_missing_blockers_and_removes_stale_sync_rows() {
    let existing = [
        ExistingDependency {
            depends_on_id: "b1",
            sync_created: true,
        },
        ExistingDependency {
            depends_on_id: "old",
            sync_created: true,
        },
        ExistingDependency {
            depends_on_id: "human",
            sync_created: false,
        },
    ];
    let blockers = [
        BlockerTask {
            id: "b1",
            key: "ABC-2",
        },
        BlockerTask {
            id: "b2",
            key: "ABC-3",
        },
    ];
    let plan = dependency_plan("ABC-1", &existing, &blockers, |_| false);
    assert_eq!(plan.add, ["b2"]);
    assert_eq!(plan.remove, ["old"]);
    assert!(plan.errors.is_empty());
}

#[test]
fn dependency_plan_refuses_blockers_that_would_cycle() {
    let blockers = [BlockerTask {
        id: "b1",
        key: "ABC-2",
    }];
    let plan = dependency_plan("ABC-1", &[], &blockers, |id| id == "b1");
    assert!(plan.add.is_empty());
    assert_eq!(
        plan.errors,
        ["ABC-1: blocked by ABC-2 would create a dependency cycle"]
    );
}

#[test]
fn dependency_plan_keeps_a_refused_existing_dependency_out_of_removal() {
    // A blocker that is already a dependency is never re-checked, and never removed.
    let existing = [ExistingDependency {
        depends_on_id: "b1",
        sync_created: true,
    }];
    let blockers = [BlockerTask {
        id: "b1",
        key: "ABC-2",
    }];
    let plan = dependency_plan("ABC-1", &existing, &blockers, |_| true);
    assert_eq!(plan, Default::default());
}
