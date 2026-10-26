//! IF-04: the administration and refresh decisions of `wg-registry` (update, remove, remove a
//! checkout, declare a dependency, refresh), as pure functions over the rows the shell loads.

#![allow(clippy::expect_used)]

use rpds::{RedBlackTreeSetSync, VectorSync};
use serde_json::Map;
use wg_model::{
    Decision, Repository, RepositoryCheckout, RepositoryDependency, Row, RowKey, Stamp, Write,
    derive_id,
};
use wg_registry::admin::{self, Patch};
use wg_registry::checkout::{KIND_CLONE, KIND_WORKTREE, WorktreeDetail, WorktreeRecord};
use wg_registry::refresh::{self, Probe};
use wg_registry::register::Matched;
use wg_types::{AccessMode, Actor, ErrorCode, Id, Permission, Timestamp, parse_id};

fn id(text: &str) -> Id {
    parse_id(text).expect("fixture id")
}

fn at(second: u8) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 12, 0, second, 0).expect("fixture time")
}

fn stamp(permissions: &[Permission]) -> Stamp {
    let entropy = [7u8; 32];
    Stamp {
        at: at(30),
        entropy,
        actor: Actor::parse("human:tester").expect("actor"),
        permissions: permissions
            .iter()
            .copied()
            .collect::<RedBlackTreeSetSync<_>>(),
        correlation: derive_id(&entropy, 0).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn admin_stamp() -> Stamp {
    stamp(&[Permission::RepoAdmin, Permission::RepoRead])
}

fn repo() -> Repository {
    Repository {
        id: id("00000000-0000-4000-8000-0000000000d1"),
        workspace_id: id("00000000-0000-4000-8000-0000000000a1"),
        name: "svc".into(),
        description: String::new(),
        identity_fingerprint: Some("aaa".into()),
        default_branch: Some("main".into()),
        tracked_branches: VectorSync::from_iter(["main".to_owned()]),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: Map::new(),
        health_checked_at: Some(at(1)),
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Map::new(),
        created_at: at(1),
        updated_at: at(1),
    }
}

fn checkout(n: u8, path: &str, kind: &str, primary: bool, available: bool) -> RepositoryCheckout {
    RepositoryCheckout {
        id: derive_id(&[n; 32], 0),
        repository_id: repo().id,
        path: path.into(),
        kind: kind.into(),
        is_primary: primary,
        access_mode: None,
        available,
        last_seen_at: Some(at(1)),
        created_at: at(n),
        updated_at: at(n),
    }
}

fn matched(checkouts: Vec<RepositoryCheckout>) -> Matched {
    Matched {
        repository: repo(),
        checkouts,
        remotes: Vec::new(),
    }
}

fn updated_repository(decision: &Decision<Id>) -> Repository {
    decision
        .writes
        .iter()
        .find_map(|write| match write {
            Write::Update {
                row: Row::Repository(row),
                ..
            } => Some(row.clone()),
            _ => None,
        })
        .expect("a repository update")
}

fn tracked(row: &Repository) -> Vec<String> {
    row.tracked_branches.iter().cloned().collect()
}

#[test]
fn update_changed_default_branch_is_tracked() {
    let patch = Patch {
        default_branch: Some("develop".into()),
        ..Patch::default()
    };
    let decision = admin::update(&admin_stamp(), &repo(), &[], &patch).expect("update");
    let row = updated_repository(&decision);
    assert_eq!(row.default_branch.as_deref(), Some("develop"));
    assert_eq!(tracked(&row), ["main", "develop"]);
    assert_eq!(decision.events.len(), 1);
}

#[test]
fn update_with_nothing_changed_writes_nothing() {
    let patch = Patch {
        description: Some(String::new()),
        default_branch: Some("main".into()),
        ..Patch::default()
    };
    let decision = admin::update(&admin_stamp(), &repo(), &[], &patch).expect("update");
    assert!(decision.writes.is_empty());
    assert!(decision.events.is_empty());
}

#[test]
fn update_replaces_the_tracked_list_and_keeps_a_given_default_in_it() {
    let patch = Patch {
        tracked_branches: Some(vec!["release".into()]),
        default_branch: Some("develop".into()),
        access_mode: Some(AccessMode::ReadOnly),
        ..Patch::default()
    };
    let row = updated_repository(&admin::update(&admin_stamp(), &repo(), &[], &patch).expect("ok"));
    assert_eq!(tracked(&row), ["release", "develop"]);
    assert_eq!(row.access_mode, AccessMode::ReadOnly);
}

#[test]
fn update_refuses_a_taken_name_and_a_missing_permission() {
    let patch = Patch {
        name: Some("other".into()),
        ..Patch::default()
    };
    let taken = vec!["other".to_owned()];
    let err = admin::update(&admin_stamp(), &repo(), &taken, &patch).expect_err("conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    let err =
        admin::update(&stamp(&[Permission::RepoRead]), &repo(), &[], &patch).expect_err("denied");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
}

#[test]
fn remove_archives_the_registration() {
    let decision = admin::remove(&admin_stamp(), &repo()).expect("remove");
    let row = updated_repository(&decision);
    assert_eq!(row.archived_at, Some(at(30)));
    assert_eq!(decision.events.len(), 1);
}

#[test]
fn remove_checkout_promotes_the_next_user_checkout() {
    let all = matched(vec![
        checkout(2, "/r/a", KIND_CLONE, true, true),
        checkout(3, "/r/managed", "managed_worktree", false, true),
        checkout(4, "/r/b", KIND_WORKTREE, false, true),
    ]);
    let decision = admin::remove_checkout(&admin_stamp(), &all, "/r/a", "/r/a").expect("ok");
    let kinds: Vec<&Write> = decision.writes.iter().collect();
    assert_eq!(kinds.len(), 2);
    assert!(matches!(
        kinds[0],
        Write::Delete(RowKey::RepositoryCheckout(removed)) if *removed == all.checkouts[0].id
    ));
    assert!(matches!(
        kinds[1],
        Write::Update { row: Row::RepositoryCheckout(next), .. }
            if next.path == "/r/b" && next.is_primary
    ));
}

#[test]
fn remove_checkout_refuses_unknown_and_managed_paths() {
    let all = matched(vec![
        checkout(2, "/r/a", KIND_CLONE, true, true),
        checkout(3, "/r/managed", "managed_worktree", false, true),
    ]);
    let err = admin::remove_checkout(&admin_stamp(), &all, "/nope", "/nope").expect_err("missing");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = admin::remove_checkout(&admin_stamp(), &all, "/r/managed", "/r/managed")
        .expect_err("managed");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
}

#[test]
fn dependencies_are_declared_once_and_never_on_oneself() {
    let other = Repository {
        id: id("00000000-0000-4000-8000-0000000000d2"),
        name: "lib".into(),
        ..repo()
    };
    let declared =
        admin::add_dependency(&admin_stamp(), &repo(), &other, None, "uses").expect("ok");
    assert_eq!(declared.writes.len(), 1);
    assert_eq!(declared.events.len(), 1);
    let row = RepositoryDependency {
        id: id("00000000-0000-4000-8000-0000000000e1"),
        from_repository_id: repo().id,
        to_repository_id: other.id,
        kind: "depends_on".into(),
        description: String::new(),
        created_at: at(1),
    };
    let again = admin::add_dependency(&admin_stamp(), &repo(), &other, Some(&row), "").expect("ok");
    assert!(again.writes.is_empty() && again.events.is_empty());
    let err = admin::add_dependency(&admin_stamp(), &repo(), &repo(), None, "").expect_err("self");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    let dropped = admin::remove_dependency(&admin_stamp(), &repo(), Some(&row)).expect("ok");
    assert!(matches!(
        dropped.writes.iter().next(),
        Some(Write::Delete(RowKey::RepositoryDependency(gone))) if *gone == row.id
    ));
}

fn probe(fingerprint: &str, remote: Option<(&str, &str)>) -> Probe {
    Probe {
        is_repository: true,
        fingerprint: Some(fingerprint.into()),
        remotes: remote
            .map(|(name, url)| (name.to_owned(), url.to_owned()))
            .into_iter()
            .collect(),
        default_branch: Some("trunk".into()),
        shallow: true,
    }
}

fn request(
    probes: Vec<Probe>,
    worktrees: Vec<(WorktreeRecord, WorktreeDetail)>,
) -> refresh::Request {
    refresh::Request {
        probes,
        worktrees,
        allowed_roots: Vec::new(),
        managed_roots: Vec::new(),
    }
}

#[test]
fn refresh_marks_missing_checkouts_and_degrades_health() {
    let all = matched(vec![
        checkout(2, "/r/a", KIND_CLONE, true, true),
        checkout(3, "/r/wt", KIND_WORKTREE, false, true),
    ]);
    let probes = vec![
        probe("aaa", Some(("origin", "https://h/x.git"))),
        Probe::default(),
    ];
    let decision = refresh::refresh(
        &request(probes, Vec::new()),
        &all,
        &stamp(&[Permission::RepoRead]),
    )
    .expect("refresh");
    let row = updated_repository(&decision);
    assert_eq!(row.health_status, "degraded");
    assert!(row.is_shallow);
    // The default branch is only filled in when the repository has none.
    assert_eq!(row.default_branch.as_deref(), Some("main"));
    // A lost worktree and the health change are events, in that order.
    assert_eq!(decision.events.len(), 2);
    // Two checkouts, one new remote, the repository.
    assert_eq!(decision.writes.len(), 4);
}

#[test]
fn refresh_treats_other_history_as_unavailable_and_discovers_worktrees() {
    let all = matched(vec![checkout(2, "/r/a", KIND_CLONE, true, true)]);
    let listed = vec![
        (
            WorktreeRecord {
                path: "/r/a".into(),
                bare: false,
                prunable: false,
            },
            WorktreeDetail {
                branch: Some("main".into()),
                detached: false,
                locked: false,
            },
        ),
        (
            WorktreeRecord {
                path: "/r/new".into(),
                bare: false,
                prunable: false,
            },
            WorktreeDetail {
                branch: None,
                detached: true,
                locked: false,
            },
        ),
    ];
    let mismatch = refresh::refresh(
        &request(vec![probe("bbb", None)], Vec::new()),
        &all,
        &stamp(&[Permission::RepoRead]),
    )
    .expect("refresh");
    assert_eq!(updated_repository(&mismatch).health_status, "unavailable");
    let found = refresh::refresh(
        &request(vec![probe("aaa", None)], listed),
        &all,
        &stamp(&[Permission::RepoRead]),
    )
    .expect("refresh");
    let inserted = found
        .writes
        .iter()
        .filter(|write| matches!(write, Write::Insert(Row::RepositoryCheckout(_))))
        .count();
    assert_eq!(inserted, 1);
    assert_eq!(updated_repository(&found).health_status, "ok");
}
