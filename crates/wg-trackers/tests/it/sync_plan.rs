//! The pure plans of the tracker sync (`wg_trackers::plan`): the cursor arithmetic, the references a
//! page still needs, the reconcile of a full sync and the pull request link changes. The expected
//! values follow `integrations/trackers/sync.py` branch by branch.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;

use wg_trackers::{
    LinkChange, LinkValues, PullRequestRef, ReconcileAction, ReconcileItem, StatusCategory,
    StoredLink, TrackerSnapshot, advanced_cursor, changed_fields, cursor_name, fetch_candidates,
    newest_update, project_slug, pull_request_changes, reconcile, referenced_keys, repository_list,
    since_seconds, timestamp_ms,
};
use wg_types::{TaskPriority, Timestamp};

const SYNC: &str = "system:tracker-sync";

fn at(hour: u8, minute: u8) -> Timestamp {
    Timestamp::from_components(2026, 10, 26, hour, minute, 0, 0).expect("timestamp")
}

fn snap(key: &str, parent: Option<&str>, blockers: &[&str]) -> TrackerSnapshot {
    TrackerSnapshot {
        external_id: format!("id-{key}"),
        key: key.to_string(),
        url: "u".to_string(),
        project_key: "WG".to_string(),
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
        remote_updated_at: at(10, 0),
        issue_type: String::new(),
        labels: Vec::new(),
        raw: serde_json::Value::Null,
    }
}

fn pr(number: u64, repo: &str, sources: &[&'static str]) -> PullRequestRef {
    PullRequestRef {
        url: format!("https://github.com/{repo}/pull/{number}"),
        normalized_url: format!("github.com/{repo}/pull/{number}"),
        provider: "github",
        repo_full_name: repo.to_string(),
        repo_normalized_url: format!("github.com/{repo}"),
        number,
        sources: sources.to_vec(),
        title: format!("PR {number}"),
    }
}

fn values(number: i64, repository: Option<&str>) -> LinkValues {
    LinkValues {
        url: format!("https://github.com/org/repo/pull/{number}"),
        provider: "github".to_string(),
        repo_full_name: "org/repo".to_string(),
        number: Some(number),
        repository_id: repository.map(str::to_string),
        sources: vec!["dev_panel".to_string()],
        title: format!("PR {number}"),
    }
}

fn stored(number: i64, created_by: &str, repository: Option<&str>) -> StoredLink {
    StoredLink {
        normalized_url: format!("github.com/org/repo/pull/{number}"),
        created_by: created_by.to_string(),
        values: values(number, repository),
    }
}

// covers: C170
#[test]
fn cursor_names_and_millisecond_timestamps_follow_the_python_helpers() {
    assert_eq!(
        cursor_name("jira1", "last_full_sync"),
        "tracker:jira1:last_full_sync"
    );
    assert_eq!(timestamp_ms(at(12, 0)), 1_793_016_000_000);
    let fractional = Timestamp::parse_iso("2026-10-26T12:00:00.123456Z").expect("timestamp");
    assert_eq!(timestamp_ms(fractional), 1_793_016_000_123);
}

#[test]
fn since_is_the_cursor_less_the_overlap_in_whole_seconds() {
    assert_eq!(since_seconds(None, 120), None);
    assert_eq!(
        since_seconds(Some(1_793_016_000_999), 120),
        Some(1_793_015_880)
    );
    assert_eq!(
        since_seconds(Some(1_793_016_000_000), 0),
        Some(1_793_016_000)
    );
}

#[test]
fn a_cursor_only_moves_forward() {
    assert_eq!(advanced_cursor(None, 500), 500);
    assert_eq!(advanced_cursor(Some(900), 500), 900);
    assert_eq!(advanced_cursor(Some(400), 500), 500);
}

#[test]
fn the_newest_update_is_taken_over_the_searched_snapshots_only() {
    assert_eq!(newest_update(&[]), None);
    let mut late = snap("WG-2", None, &[]);
    late.remote_updated_at = at(14, 30);
    let early = snap("WG-1", None, &[]);
    assert_eq!(newest_update(&[early, late]), Some(at(14, 30)));
}

#[test]
fn referenced_keys_are_the_parent_then_the_blockers() {
    let item = snap("WG-9", Some("WG-1"), &["WG-2", "OPS-3"]);
    assert_eq!(referenced_keys(&item), vec!["WG-1", "WG-2", "OPS-3"]);
    assert!(referenced_keys(&snap("WG-9", None, &[])).is_empty());
}

#[test]
fn fetch_candidates_skip_held_keys_and_other_projects_and_repeats() {
    let have: HashSet<String> = ["WG-1".to_string()].into_iter().collect();
    let projects = vec!["WG".to_string()];
    let wanted = fetch_candidates(&["WG-1", "WG-7", "OPS-3", "WG-7", "WG-5"], &have, &projects);
    assert_eq!(wanted, vec!["WG-7".to_string(), "WG-5".to_string()]);
}

#[test]
fn project_keys_map_to_lowercase_slugs() {
    assert_eq!(project_slug("WG"), "wg");
    assert_eq!(project_slug("Ops2"), "ops2");
}

#[test]
fn changed_fields_are_sorted_and_unique() {
    assert_eq!(
        changed_fields(&["title", "assignee", "title", "status"]),
        vec!["assignee", "status", "title"]
    );
}

#[test]
fn repository_list_keeps_first_seen_registered_repositories_once() {
    let resolved = vec![
        Some("repo-b".to_string()),
        None,
        Some("repo-a".to_string()),
        Some("repo-b".to_string()),
    ];
    assert_eq!(repository_list(&resolved), vec!["repo-b", "repo-a"]);
}

fn listed(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn a_listed_item_is_restored_and_unarchived_only_when_it_was_marked() {
    let items = [
        ReconcileItem {
            external_id: "a",
            task_archived: Some(true),
            missing_since: Some(at(9, 0)),
        },
        ReconcileItem {
            external_id: "b",
            task_archived: Some(true),
            missing_since: None,
        },
        ReconcileItem {
            external_id: "c",
            task_archived: None,
            missing_since: Some(at(9, 0)),
        },
    ];
    let actions = reconcile(&items, &listed(&["a", "b", "c"]), at(12, 0), 3600);
    assert_eq!(
        actions,
        vec![
            ReconcileAction::Restored {
                clear_missing: true,
                unarchive: true
            },
            ReconcileAction::Restored {
                clear_missing: false,
                unarchive: false
            },
            ReconcileAction::Untouched,
        ]
    );
}

#[test]
fn an_unlisted_item_is_marked_now_and_archived_only_after_the_grace_period() {
    let fresh = ReconcileItem {
        external_id: "x",
        task_archived: Some(false),
        missing_since: None,
    };
    let old_mark = ReconcileItem {
        external_id: "y",
        task_archived: Some(false),
        missing_since: Some(at(9, 0)),
    };
    let archived_already = ReconcileItem {
        external_id: "z",
        task_archived: Some(true),
        missing_since: Some(at(9, 0)),
    };
    let actions = reconcile(
        &[fresh, old_mark, archived_already],
        &listed(&[]),
        at(12, 0),
        3600,
    );
    assert_eq!(
        actions,
        vec![
            ReconcileAction::Missing {
                since: at(12, 0),
                archive: false
            },
            ReconcileAction::Missing {
                since: at(9, 0),
                archive: true
            },
            ReconcileAction::Missing {
                since: at(9, 0),
                archive: false
            },
        ]
    );
}

#[test]
fn the_grace_period_is_strict_so_a_mark_exactly_grace_old_is_not_archived() {
    let item = ReconcileItem {
        external_id: "y",
        task_archived: Some(false),
        missing_since: Some(at(11, 0)),
    };
    let actions = reconcile(&[item], &listed(&[]), at(12, 0), 3600);
    assert_eq!(
        actions,
        vec![ReconcileAction::Missing {
            since: at(11, 0),
            archive: false
        }]
    );
}

#[test]
fn a_new_pull_request_is_inserted_with_its_repository() {
    let wanted = vec![(
        pr(42, "org/repo", &["dev_panel"]),
        Some("repo-1".to_string()),
    )];
    let changes = pull_request_changes(&[], &wanted);
    assert_eq!(
        changes,
        vec![LinkChange::Insert {
            normalized_url: "github.com/org/repo/pull/42".to_string(),
            values: LinkValues {
                url: "https://github.com/org/repo/pull/42".to_string(),
                provider: "github".to_string(),
                repo_full_name: "org/repo".to_string(),
                number: Some(42),
                repository_id: Some("repo-1".to_string()),
                sources: vec!["dev_panel".to_string()],
                title: "PR 42".to_string(),
            },
        }]
    );
}

#[test]
fn a_changed_link_is_updated_and_an_identical_one_is_left_alone() {
    let stored_links = vec![stored(42, SYNC, None), stored(43, SYNC, None)];
    let wanted = vec![
        (
            pr(42, "org/repo", &["dev_panel"]),
            Some("repo-1".to_string()),
        ),
        (pr(43, "org/repo", &["dev_panel"]), None),
    ];
    let changes = pull_request_changes(&stored_links, &wanted);
    assert_eq!(changes.len(), 1);
    assert!(matches!(
        &changes[0],
        LinkChange::Update { index: 0, values } if values.repository_id.as_deref() == Some("repo-1")
    ));
}

#[test]
fn sync_created_links_no_longer_named_are_deleted_and_human_ones_are_kept() {
    let stored_links = vec![stored(7, SYNC, None), stored(8, "human:Brice", None)];
    let changes = pull_request_changes(&stored_links, &[]);
    assert_eq!(changes, vec![LinkChange::Delete { index: 0 }]);
}

#[test]
fn the_first_reference_to_a_url_wins_and_matching_is_by_normalized_url() {
    let wanted = vec![
        (pr(5, "org/repo", &["custom_field"]), None),
        (
            pr(5, "org/repo", &["dev_panel"]),
            Some("repo-9".to_string()),
        ),
    ];
    let changes = pull_request_changes(&[], &wanted);
    assert_eq!(changes.len(), 1);
    assert!(matches!(
        &changes[0],
        LinkChange::Insert { values, .. } if values.sources == vec!["custom_field".to_string()]
            && values.repository_id.is_none()
    ));
}
