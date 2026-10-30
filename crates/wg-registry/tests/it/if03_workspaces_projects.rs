//! IF-03: the workspace, project and rename decisions of `wg-registry` over registry slices.

use serde_json::{Map, Value, json};
use wg_model::{
    JsonObject, Project, RegistrySlice, Repository, Stamp, Workspace, Write, derive_id,
};
use wg_registry::{projects, renames, workspaces};
use wg_types::{
    AccessMode, Actor, ActorKind, ErrorCode, Id, Permission, Timestamp, WgError, human_permissions,
};

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("unexpected failure: {err}"),
    }
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn stamp(seed: u8, second: u8, permissions: &[Permission]) -> Stamp {
    Stamp {
        at: at(second),
        entropy: entropy(seed),
        actor: ok(Actor::new(ActorKind::Human, "brice")),
        permissions: permissions.iter().copied().collect(),
        correlation: derive_id(&entropy(99), 0).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn human(seed: u8) -> Stamp {
    stamp(
        seed,
        seed,
        &human_permissions().into_iter().collect::<Vec<_>>(),
    )
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn ws(slice: &RegistrySlice, decision_writes: &wg_model::Decision<impl Sized>) -> RegistrySlice {
    slice.apply_all(decision_writes.writes.iter())
}

fn workspace_dump(slug: &str) -> JsonObject {
    object(json!({"slug": slug, "name": "Acme", "description": "", "key_prefix": "AC"}))
}

fn repository(n: u32, name: &str, archived: bool) -> Repository {
    Repository {
        id: derive_id(&entropy(2), n),
        workspace_id: derive_id(&entropy(1), 0),
        name: name.into(),
        description: format!("The {name} repo"),
        identity_fingerprint: None,
        default_branch: None,
        tracked_branches: Default::default(),
        access_mode: AccessMode::ReadWrite,
        health_status: "unknown".into(),
        health_detail: Map::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: archived.then(|| at(0)),
        metadata: Map::new(),
        created_at: at(0),
        updated_at: at(0),
    }
}

/// A slice with the workspace `acme`, the repositories `core` and `old` (archived) and no project.
fn world() -> (RegistrySlice, Id) {
    let created = ok(workspaces::create(
        &RegistrySlice::default(),
        &human(1),
        &workspace_dump("acme"),
    ));
    let acme = created.reply.id;
    let slice = ws(&RegistrySlice::default(), &created)
        .apply(&Write::insert(repository(1, "core", false)))
        .apply(&Write::insert(repository(2, "old", true)));
    (slice, acme)
}

fn code(result: Result<impl Sized, WgError>) -> ErrorCode {
    match result {
        Ok(_) => panic!("expected a failure"),
        Err(err) => err.code,
    }
}

// covers: C181
#[test]
fn a_workspace_slug_can_be_used_once() {
    let (slice, _) = world();
    let again = workspaces::create(&slice, &human(3), &workspace_dump("acme"));
    assert_eq!(code(again), ErrorCode::Conflict);
    let denied = workspaces::create(&slice, &stamp(3, 3, &[]), &workspace_dump("other"));
    assert_eq!(code(denied), ErrorCode::PermissionDenied);
}

// covers: C183
#[test]
fn settings_merge_and_an_unchanged_merge_keeps_updated_at() {
    let (slice, _) = world();
    let roots = object(json!({"allowed_roots": ["/x"]}));
    let first = ok(workspaces::update_settings(
        &slice,
        &human(4),
        "acme",
        &roots,
    ));
    assert_eq!(first.reply.settings["allowed_roots"], json!(["/x"]));
    assert_eq!(first.reply.updated_at, at(4));
    let after = ws(&slice, &first);
    let second = ok(workspaces::update_settings(
        &after,
        &human(5),
        "acme",
        &roots,
    ));
    assert_eq!(second.reply.updated_at, at(4));
    assert!(second.writes.is_empty());
    assert_eq!(second.events.len(), 1);
    let bad = object(json!({"nope": 1}));
    assert_eq!(
        code(workspaces::update_settings(&slice, &human(6), "acme", &bad)),
        ErrorCode::ValidationFailed
    );
}

// covers: C180
#[test]
fn archiving_sets_archived_at() {
    let (slice, _) = world();
    let archived = ok(workspaces::archive(&slice, &human(7), "acme"));
    assert_eq!(archived.reply.archived_at, Some(at(7)));
    assert_eq!(
        code(workspaces::archive(&slice, &human(7), "nope")),
        ErrorCode::NotFound
    );
}

fn project_dump(slug: &str, repositories: &[&str]) -> JsonObject {
    object(json!({
        "slug": slug, "name": "Core", "description": "", "repositories": repositories,
        "settings": {}
    }))
}

// covers: C109, C108, C111
#[test]
fn projects_hold_repositories_and_archived_ones_are_not_found_to_add() {
    let (slice, acme) = world();
    let created = ok(projects::create(
        &slice,
        &human(8),
        acme,
        &project_dump("core", &["core"]),
    ));
    assert_eq!(created.reply.repository_ids.len(), 1);
    assert_eq!(created.events.len(), 1);
    let slice = ws(&slice, &created);
    let taken = projects::create(&slice, &human(9), acme, &project_dump("core", &[]));
    assert_eq!(code(taken), ErrorCode::Conflict);
    let missing = projects::create(&slice, &human(9), acme, &project_dump("x", &["old"]));
    assert_eq!(code(missing), ErrorCode::NotFound);

    let repeat = ok(projects::add_repository(
        &slice,
        &human(10),
        acme,
        "core",
        "core",
        "member",
    ));
    assert!(repeat.writes.is_empty() && repeat.events.is_empty());
    let archived = projects::add_repository(&slice, &human(10), acme, "core", "old", "member");
    assert_eq!(code(archived), ErrorCode::NotFound);

    let removed = ok(projects::remove_repository(
        &slice,
        &human(11),
        acme,
        "core",
        "core",
    ));
    assert!(removed.reply.repository_ids.is_empty());
    assert_eq!(removed.events.len(), 1);
    let none = ok(projects::remove_repository(
        &ws(&slice, &removed),
        &human(12),
        acme,
        "core",
        "old",
    ));
    assert!(none.events.is_empty());
}

// covers: C114
#[test]
fn an_update_changes_only_what_differs() {
    let (slice, acme) = world();
    let created = ok(projects::create(
        &slice,
        &human(8),
        acme,
        &project_dump("core", &[]),
    ));
    let other = ok(projects::create(
        &ws(&slice, &created),
        &human(9),
        acme,
        &project_dump("other", &[]),
    ));
    let slice = ws(&ws(&slice, &created), &other);
    let same = object(json!({"name": "Core", "description": null, "status": null}));
    let unchanged = ok(projects::update(&slice, &human(10), acme, "core", &same));
    assert!(unchanged.writes.is_empty() && unchanged.events.is_empty());
    let change = object(json!({"name": "Kernel", "status": "archived"}));
    let changed = ok(projects::update(&slice, &human(11), acme, "core", &change));
    assert_eq!(changed.reply.project.name, "Kernel");
    assert_eq!(changed.reply.project.status, "archived");
    assert_eq!(changed.reply.project.updated_at, at(11));
    let clash = object(json!({"slug": "other"}));
    assert_eq!(
        code(projects::update(&slice, &human(12), acme, "core", &clash)),
        ErrorCode::Conflict
    );
}

// covers: C112
#[test]
fn a_repository_rename_checks_the_name_is_free() {
    let (slice, acme) = world();
    let core = derive_id(&entropy(2), 1);
    let renamed = ok(renames::rename_repository(
        &slice,
        &human(13),
        acme,
        core,
        Some("kernel"),
        None,
    ));
    assert_eq!(renamed.events.len(), 1);
    let taken = renames::rename_repository(&slice, &human(13), acme, core, Some("old"), None);
    assert_eq!(code(taken), ErrorCode::Conflict);
    let nothing = ok(renames::rename_repository(
        &slice,
        &human(13),
        acme,
        core,
        Some("core"),
        Some("The core repo"),
    ));
    assert!(nothing.events.is_empty());
}

#[test]
fn project_rows_keep_their_workspace() {
    let (slice, acme) = world();
    let created = ok(projects::create(
        &slice,
        &human(8),
        acme,
        &project_dump("core", &[]),
    ));
    let project: &Project = &created.reply.project;
    assert_eq!(project.workspace_id, acme);
    let _: Option<&Workspace> = slice.workspace(acme);
}
