//! Reference resolution: the cases of `resolve.py` (and the multi-workspace isolation of
//! `tests/unit/test_tasks_core.py`) as pure lookups over slices.

use wg_model::{JsonObject, Project, RegistrySlice, Repository, Task, TaskSlice, Workspace, Write};
use wg_registry::resolve;
use wg_types::{
    AccessMode, ErrorCode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError, parse_id,
};

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn id(text: &str) -> Id {
    ok(parse_id(text))
}

fn at() -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, 0, 0))
}

const WS_A: &str = "00000000-0000-4000-8000-0000000000a1";
const WS_B: &str = "00000000-0000-4000-8000-0000000000b1";
const PROJECT_A: &str = "00000000-0000-4000-8000-0000000000c1";
const PROJECT_B: &str = "00000000-0000-4000-8000-0000000000c2";
const REPO: &str = "00000000-0000-4000-8000-0000000000d1";
const REPO_ARCHIVED: &str = "00000000-0000-4000-8000-0000000000d2";
const REPO_B: &str = "00000000-0000-4000-8000-0000000000d3";
const TASK_A1: &str = "00000000-0000-4000-8000-0000000000e1";
const TASK_B1: &str = "00000000-0000-4000-8000-0000000000e2";
const TASK_A2: &str = "00000000-0000-4000-8000-0000000000e3";

fn workspace(text: &str, slug: &str, prefix: &str) -> Workspace {
    Workspace {
        id: id(text),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        key_prefix: prefix.into(),
        next_task_number: 3,
        settings: JsonObject::new(),
        archived_at: None,
        created_at: at(),
        updated_at: at(),
    }
}

fn project(text: &str, workspace_id: &str, slug: &str) -> Project {
    Project {
        id: id(text),
        workspace_id: id(workspace_id),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        status: "active".into(),
        settings: JsonObject::new(),
        created_at: at(),
        updated_at: at(),
    }
}

fn repository(text: &str, workspace_id: &str, name: &str, archived: bool) -> Repository {
    Repository {
        id: id(text),
        workspace_id: id(workspace_id),
        name: name.into(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: None,
        tracked_branches: Default::default(),
        access_mode: AccessMode::ReadWrite,
        health_status: "unknown".into(),
        health_detail: JsonObject::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: archived.then(at),
        metadata: JsonObject::new(),
        created_at: at(),
        updated_at: at(),
    }
}

fn task(text: &str, workspace_id: &str, key: &str) -> Task {
    Task {
        id: id(text),
        workspace_id: id(workspace_id),
        number: 1,
        key: ok(TaskKey::parse(key)),
        title: key.into(),
        description: String::new(),
        description_hash: String::new(),
        acceptance_criteria: Default::default(),
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: JsonObject::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(),
        updated_at: at(),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

/// Two workspaces, `default` (WG) and `other` (OT). Both hold a `WG-1`, so the key is ambiguous
/// across workspaces.
fn registry() -> RegistrySlice {
    RegistrySlice::default().apply_all(&[
        Write::insert(workspace(WS_A, "default", "WG")),
        Write::insert(workspace(WS_B, "other", "OT")),
        Write::insert(project(PROJECT_A, WS_A, "alpha")),
        Write::insert(project(PROJECT_B, WS_B, "beta")),
        Write::insert(repository(REPO, WS_A, "api", false)),
        Write::insert(repository(REPO_ARCHIVED, WS_A, "old-api", true)),
        Write::insert(repository(REPO_B, WS_B, "api", false)),
    ])
}

fn tasks() -> TaskSlice {
    TaskSlice {
        registry: registry(),
        ..TaskSlice::default()
    }
    .apply_all(&[
        Write::insert(task(TASK_A1, WS_A, "WG-1")),
        Write::insert(task(TASK_A2, WS_A, "WG-2")),
        Write::insert(task(TASK_B1, WS_B, "OT-1")),
    ])
}

fn tasks_with_shared_key() -> TaskSlice {
    tasks().apply(&Write::insert(task(TASK_B1, WS_B, "WG-1")))
}

fn not_found_detail(err: &WgError) -> Option<&serde_json::Value> {
    err.details.get("ref")
}

#[test]
fn workspace_by_uuid_slug_and_miss() {
    let slice = registry();
    assert_eq!(ok(resolve::workspace(&slice, WS_B)).slug, "other");
    assert_eq!(ok(resolve::workspace(&slice, "default")).key_prefix, "WG");
    // A UUID that misses falls through to the slug lookup, as in Python.
    let err = ok_err(resolve::workspace(&slice, "missing"));
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.message, "workspace 'missing' not found");
}

#[test]
fn workspace_not_found_carries_ref_detail() {
    let slice = registry();
    let err = ok_err(resolve::workspace(&slice, "nope"));
    assert_eq!(err.message, "workspace 'nope' not found");
    assert_eq!(not_found_detail(&err), Some(&serde_json::json!("nope")));
}

#[test]
fn project_by_uuid_and_slug_within_workspace() {
    let slice = registry();
    let by_id = ok(resolve::project(&slice, id(WS_A), PROJECT_A));
    assert_eq!(by_id.slug, "alpha");
    assert_eq!(
        ok(resolve::project(&slice, id(WS_A), "alpha")).id,
        id(PROJECT_A)
    );
}

#[test]
fn project_of_another_workspace_is_not_found() {
    let slice = registry();
    // By UUID: the row exists, but in workspace B.
    let err = ok_err(resolve::project(&slice, id(WS_A), PROJECT_B));
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        err.message,
        format!("project '{PROJECT_B}' not found in workspace")
    );
    // By slug: `beta` belongs to workspace B only.
    let err = ok_err(resolve::project(&slice, id(WS_A), "beta"));
    assert_eq!(err.message, "project 'beta' not found in workspace");
    assert_eq!(not_found_detail(&err), Some(&serde_json::json!("beta")));
}

#[test]
fn repository_by_name_not_slug_and_uuid() {
    let slice = registry();
    assert_eq!(
        ok(resolve::repository(&slice, id(WS_A), "api", false)).id,
        id(REPO)
    );
    assert_eq!(
        ok(resolve::repository(&slice, id(WS_A), REPO, false)).name,
        "api"
    );
    // Other workspace's `api` is not visible from workspace A by UUID.
    let err = ok_err(resolve::repository(&slice, id(WS_A), REPO_B, false));
    assert_eq!(err.code, ErrorCode::NotFound);
    // Slugs are not names for repositories; a name lookup of a missing repository is not found.
    let err = ok_err(resolve::repository(&slice, id(WS_A), "missing", false));
    assert_eq!(err.message, "repository 'missing' not found in workspace");
}

#[test]
fn archived_repository_needs_include_archived() {
    let slice = registry();
    let err = ok_err(resolve::repository(&slice, id(WS_A), "old-api", false));
    assert_eq!(err.message, "repository 'old-api' not found in workspace");
    assert_eq!(
        ok(resolve::repository(&slice, id(WS_A), "old-api", true)).id,
        id(REPO_ARCHIVED)
    );
    // Archived by UUID, without the flag, is also not found.
    let err = ok_err(resolve::repository(&slice, id(WS_A), REPO_ARCHIVED, false));
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn task_by_uuid_and_key_is_case_insensitive_and_trimmed() {
    let slice = tasks();
    assert_eq!(
        ok(resolve::task(&slice, None, TASK_A2)).key.to_string(),
        "WG-2"
    );
    assert_eq!(
        ok(resolve::task(&slice, None, "wg-2")).key.to_string(),
        "WG-2"
    );
    assert_eq!(
        ok(resolve::task(&slice, Some(id(WS_A)), "  Wg-1  ")).title,
        "WG-1"
    );
    assert_eq!(
        ok(resolve::task(&slice, Some(id(WS_B)), "OT-1")).title,
        "OT-1"
    );
}

#[test]
fn empty_task_reference_is_a_validation_error() {
    let slice = tasks();
    let err = ok_err(resolve::task(&slice, None, ""));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(err.message, "task reference must not be empty");
}

#[test]
fn task_key_shared_across_workspaces_is_ambiguous_without_a_workspace() {
    let slice = tasks_with_shared_key();
    let err = ok_err(resolve::task(&slice, None, "wg-1"));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(
        err.message,
        "task key 'wg-1' is ambiguous across workspaces; pass a workspace"
    );
    // With a workspace, the key is unambiguous.
    assert_eq!(
        ok(resolve::task(&slice, Some(id(WS_B)), "WG-1")).id,
        id(TASK_B1)
    );
    assert_eq!(
        ok(resolve::task(&slice, Some(id(WS_A)), "WG-1")).id,
        id(TASK_A1)
    );
}

#[test]
fn task_of_another_workspace_is_not_found() {
    let slice = tasks();
    // By UUID: the task exists in workspace B.
    let err = ok_err(resolve::task(&slice, Some(id(WS_A)), TASK_B1));
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.message, format!("task '{TASK_B1}' not found"));
    // By key: OT-1 is not a key of workspace A.
    let err = ok_err(resolve::task(&slice, Some(id(WS_A)), "OT-1"));
    assert_eq!(err.message, "task 'OT-1' not found");
    assert_eq!(not_found_detail(&err), Some(&serde_json::json!("OT-1")));
    // A missing key with no workspace.
    let err = ok_err(resolve::task(&slice, None, "WG-404"));
    assert_eq!(err.message, "task 'WG-404' not found");
}

#[test]
fn python_repr_quotes_the_reference_in_messages() {
    let slice = tasks();
    let err = ok_err(resolve::task(&slice, None, "it's"));
    assert_eq!(err.message, "task \"it's\" not found");
}

fn ok_err<T: core::fmt::Debug>(result: Result<T, WgError>) -> WgError {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(err) => err,
    }
}
