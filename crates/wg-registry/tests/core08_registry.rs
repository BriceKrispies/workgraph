//! CORE-08: the identity, checkout discovery and health decisions of `wg-registry`, as pure
//! functions over the values the shell reads.

use wg_model::RepositoryCheckout;
use wg_registry::checkout::{
    Discovery, KIND_CLONE, KIND_MANAGED_WORKTREE, KIND_WORKTREE, RemoteWrite, WorktreeRecord,
    check_boundary, discover_worktrees, is_within, plan_remote_writes, primary_checkout,
};
use wg_registry::health::{Observation, health_status, observe, with_prunable};
use wg_registry::identity::{
    MatchedBy, fingerprint, history_mismatch, matched_by, new_repository_name,
};
use wg_types::{ErrorCode, Id, Timestamp, parse_id};

fn id(text: &str) -> Id {
    match parse_id(text) {
        Ok(id) => id,
        Err(err) => panic!("invalid fixture id {text}: {err}"),
    }
}

fn at(second: u8) -> Timestamp {
    match Timestamp::from_components(2026, 10, 25, 12, 0, second, 0) {
        Ok(at) => at,
        Err(err) => panic!("invalid fixture time: {err}"),
    }
}

fn checkout(
    path: &str,
    kind: &str,
    is_primary: bool,
    available: bool,
    second: u8,
) -> RepositoryCheckout {
    RepositoryCheckout {
        id: id("00000000-0000-4000-8000-0000000000f1"),
        repository_id: id("00000000-0000-4000-8000-0000000000d1"),
        path: path.into(),
        kind: kind.into(),
        is_primary,
        access_mode: None,
        available,
        last_seen_at: None,
        created_at: at(second),
        updated_at: at(second),
    }
}

fn record(path: &str) -> WorktreeRecord {
    WorktreeRecord {
        path: path.into(),
        bare: false,
        prunable: false,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

// --- identity ---------------------------------------------------------------------------------

#[test]
fn fingerprint_sorts_dedups_and_is_none_when_empty() {
    assert_eq!(
        fingerprint(&strings(&["bbb", "aaa", "bbb"])).as_deref(),
        Some("aaa,bbb")
    );
    assert_eq!(fingerprint(&[]), None);
}

#[test]
fn history_mismatch_ignores_missing_values() {
    assert!(history_mismatch(Some("aaa"), Some("bbb")));
    assert!(!history_mismatch(Some("aaa"), Some("aaa")));
    assert!(!history_mismatch(None, Some("bbb")));
    assert!(!history_mismatch(Some("aaa"), None));
    assert!(!history_mismatch(Some(""), Some("bbb")));
}

#[test]
fn match_prefers_path_then_fingerprint_then_remote() {
    let a = id("00000000-0000-4000-8000-0000000000a1");
    let b = id("00000000-0000-4000-8000-0000000000a2");
    let c = id("00000000-0000-4000-8000-0000000000a3");
    assert_eq!(
        matched_by(Some(a), Some(b), Some(c)),
        Some((a, MatchedBy::Path))
    );
    assert_eq!(
        matched_by(None, Some(b), Some(c)),
        Some((b, MatchedBy::Fingerprint))
    );
    assert_eq!(
        matched_by(None, None, Some(c)),
        Some((c, MatchedBy::Remote))
    );
    assert_eq!(matched_by(None, None, None), None);
    assert_eq!(MatchedBy::Fingerprint.as_str(), "fingerprint");
}

#[test]
fn new_name_uses_requested_then_directory_then_suffixes() {
    let name =
        |requested: Option<&str>, taken: &[&str]| new_repository_name(requested, "dir", taken).ok();
    assert_eq!(name(Some("core"), &[]).as_deref(), Some("core"));
    assert_eq!(name(None, &[]).as_deref(), Some("dir"));
    assert_eq!(name(Some(""), &[]).as_deref(), Some("dir"));
    assert_eq!(name(None, &["dir", "dir-2"]).as_deref(), Some("dir-3"));
    let err = new_repository_name(Some("core"), "dir", &["core"]).expect_err("taken");
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.message, "repository name 'core' is already registered");
}

// --- boundaries and paths ---------------------------------------------------------------------

#[test]
fn is_within_is_component_wise() {
    assert!(is_within("/work/app", "/work"));
    assert!(is_within("/work", "/work"));
    assert!(!is_within("/workspace", "/work"));
    assert!(is_within(r"C:\dev\app", "C:/dev"));
    assert!(is_within("/anything", "/"));
}

#[test]
fn boundary_enforces_allowed_roots_and_managed_dirs() {
    let roots = strings(&["/work"]);
    let managed = strings(&["/work/.wg/worktrees"]);
    assert!(check_boundary("/work/app", &roots, &managed).is_ok());
    assert!(check_boundary("/elsewhere", &[], &managed).is_ok());

    let outside = check_boundary("/elsewhere", &roots, &managed).expect_err("outside");
    assert_eq!(outside.code, ErrorCode::ValidationFailed);
    assert_eq!(
        outside.message,
        "/elsewhere is outside the workspace's allowed roots"
    );

    let inside_managed =
        check_boundary("/work/.wg/worktrees/x", &roots, &managed).expect_err("managed");
    assert_eq!(
        inside_managed.message,
        "Workgraph-managed directories cannot be registered as repositories"
    );
}

// --- checkout discovery -----------------------------------------------------------------------

#[test]
fn discovery_kinds_follow_position_and_skips_are_reported() {
    let records = vec![
        record("/work/main"),
        WorktreeRecord {
            path: "/work/bare".into(),
            bare: true,
            prunable: false,
        },
        record("/work/feature"),
        WorktreeRecord {
            path: "/work/gone".into(),
            bare: false,
            prunable: true,
        },
        record("/work/.wg/worktrees/managed"),
        record("/elsewhere/tree"),
        record("/work/feature"),
    ];
    let decisions = discover_worktrees(
        &records,
        &[],
        &strings(&["/work"]),
        &strings(&["/work/.wg/worktrees"]),
    );
    assert_eq!(
        decisions,
        vec![
            Discovery::Add {
                path: "/work/main".into(),
                kind: KIND_CLONE
            },
            Discovery::Add {
                path: "/work/feature".into(),
                kind: KIND_WORKTREE
            },
            Discovery::Skip {
                path: "/work/gone".into(),
                reason: "worktree is prunable".into()
            },
            Discovery::Skip {
                path: "/elsewhere/tree".into(),
                reason: "/elsewhere/tree is outside the workspace's allowed roots".into()
            },
        ]
    );
}

#[test]
fn discovery_drops_known_paths() {
    let records = vec![record("/work/main"), record("/work/feature")];
    let decisions = discover_worktrees(&records, &strings(&["/work/main"]), &[], &[]);
    assert_eq!(
        decisions,
        vec![Discovery::Add {
            path: "/work/feature".into(),
            kind: KIND_WORKTREE
        }]
    );
}

#[test]
fn primary_checkout_skips_managed_and_prefers_primary_then_age() {
    let rows = vec![
        checkout("/managed", KIND_MANAGED_WORKTREE, true, true, 1),
        checkout("/old-secondary", KIND_WORKTREE, false, true, 2),
        checkout("/primary-unavailable", KIND_CLONE, true, false, 3),
        checkout("/primary", KIND_CLONE, true, true, 4),
    ];
    assert_eq!(
        primary_checkout(&rows, true).map(|c| c.path.as_str()),
        Some("/primary")
    );
    assert_eq!(
        primary_checkout(&rows, false).map(|c| c.path.as_str()),
        Some("/primary-unavailable")
    );
    let none = vec![checkout("/gone", KIND_CLONE, true, false, 1)];
    assert!(primary_checkout(&none, true).is_none());
}

#[test]
fn remote_writes_mark_existing_names_as_updates() {
    let remotes = vec![
        (
            "origin".to_owned(),
            "git@github.com:Org/Repo.git".to_owned(),
        ),
        (
            "upstream".to_owned(),
            "https://github.com/Up/Repo".to_owned(),
        ),
    ];
    let writes = plan_remote_writes(&remotes, &["origin"]);
    assert_eq!(
        writes,
        vec![
            RemoteWrite {
                name: "origin".into(),
                url: "git@github.com:Org/Repo.git".into(),
                normalized_url: "github.com/Org/Repo".into(),
                update: true,
            },
            RemoteWrite {
                name: "upstream".into(),
                url: "https://github.com/Up/Repo".into(),
                normalized_url: "github.com/Up/Repo".into(),
                update: false,
            },
        ]
    );
}

// --- health -----------------------------------------------------------------------------------

#[test]
fn observation_maps_probe_facts_to_errors() {
    assert_eq!(
        observe(true, Some("aaa"), Some("aaa")),
        Observation::Available
    );
    assert_eq!(
        observe(false, Some("aaa"), None),
        Observation::NotRepository
    );
    assert_eq!(
        observe(true, Some("aaa"), Some("bbb")),
        Observation::HistoryMismatch
    );
    assert_eq!(Observation::Available.error(), None);
    assert_eq!(
        Observation::NotRepository.error(),
        Some("path missing or not a git repository")
    );
    assert!(!Observation::HistoryMismatch.available());
}

#[test]
fn prunable_overrides_only_linked_worktrees() {
    assert_eq!(
        with_prunable(Observation::Available, KIND_WORKTREE, true),
        Observation::Prunable
    );
    assert_eq!(
        with_prunable(Observation::Available, KIND_CLONE, true),
        Observation::Available
    );
    assert_eq!(
        with_prunable(Observation::NotRepository, KIND_WORKTREE, false),
        Observation::NotRepository
    );
}

#[test]
fn status_is_ok_degraded_or_unavailable() {
    let all_up = vec![
        checkout("/a", KIND_CLONE, true, true, 1),
        checkout("/b", KIND_WORKTREE, false, true, 2),
        checkout("/managed", KIND_MANAGED_WORKTREE, false, false, 3),
    ];
    assert_eq!(health_status(&all_up), "ok");
    let one_down = vec![
        checkout("/a", KIND_CLONE, true, true, 1),
        checkout("/b", KIND_WORKTREE, false, false, 2),
    ];
    assert_eq!(health_status(&one_down), "degraded");
    let none_up = vec![checkout("/a", KIND_CLONE, true, false, 1)];
    assert_eq!(health_status(&none_up), "unavailable");
    assert_eq!(health_status(&[]), "unavailable");
}
