//! LIB-12: the DTOs of the workspace, project, repository and worktree area, the MCP list shapes,
//! and the summary counts.

#![allow(clippy::expect_used)]

use serde_json::Map;
use wg_json::{Value, http_json};
use wg_model::entity::repository::{Repository, RepositoryCheckout, RepositoryRemote};
use wg_model::entity::workspace::Workspace;
use wg_types::{AccessMode, Id, Timestamp, parse_id};
use wg_wire::dto::repository_status::Status;
use wg_wire::dto::{
    mcp_shapes, project_summary, repository, repository_brief, repository_checkout,
    repository_remote, repository_status, workspace, workspace_summary,
};

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 13, 56, 0, micros).expect("timestamp")
}

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        other => panic!("not a model: {other:?}"),
    }
}

fn sample_workspace() -> Workspace {
    Workspace {
        id: id_of("00000000-0000-4000-8000-000000000001"),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 1,
        settings: Map::new(),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn sample_repository() -> Repository {
    Repository {
        id: id_of("00000000-0000-4000-8000-0000000000a1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        name: "wg".into(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: Some("main".into()),
        tracked_branches: Default::default(),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: Map::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Map::new(),
        created_at: at(0),
        updated_at: at(0),
    }
}

fn sample_remote(repository: &Repository) -> RepositoryRemote {
    RepositoryRemote {
        id: id_of("00000000-0000-4000-8000-0000000000b1"),
        repository_id: repository.id,
        name: "origin".into(),
        url: "git@example.com:wg.git".into(),
        normalized_url: "example.com/wg".into(),
        created_at: at(0),
        updated_at: at(0),
    }
}

fn sample_checkout(repository: &Repository) -> RepositoryCheckout {
    RepositoryCheckout {
        id: id_of("00000000-0000-4000-8000-0000000000c1"),
        repository_id: repository.id,
        path: "C:/dev/wg".into(),
        kind: "clone".into(),
        is_primary: true,
        access_mode: None,
        available: true,
        last_seen_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

#[test]
fn area_dtos_list_their_fields_in_order() {
    let ws = sample_workspace();
    let repo = sample_repository();
    let remotes = [sample_remote(&repo)];
    let checkouts = [sample_checkout(&repo)];

    assert_eq!(
        names(&workspace_summary::from_model(
            &ws,
            &workspace_summary::Counts::default()
        )),
        workspace_summary::FIELDS
    );
    assert_eq!(
        names(&project_summary::from_model(
            Value::Null,
            &[],
            &[],
            Vec::new()
        )),
        project_summary::FIELDS
    );
    assert_eq!(
        names(&repository::from_model(&repo, &remotes, &checkouts, [], [])),
        repository::FIELDS
    );
    assert_eq!(
        names(&repository_remote::from_model(&remotes[0])),
        repository_remote::FIELDS
    );
    assert_eq!(
        names(&repository_checkout::from_model(&checkouts[0])),
        repository_checkout::FIELDS
    );
    assert_eq!(
        names(&repository_brief::from_model(&repo)),
        repository_brief::FIELDS
    );
    assert_eq!(
        names(&repository_status::from_model(
            repo.id,
            "wg",
            &Status::default()
        )),
        repository_status::FIELDS
    );
}

#[test]
fn workspace_summary_reads_ready_and_prints_counts() {
    let counts = workspace_summary::Counts {
        projects: 2,
        repositories: 1,
        tasks_by_status: vec![("ready".into(), 3), ("blocked".into(), 1)],
        running_attempts: 0,
        queued_jobs: 4,
        dead_jobs: 0,
    };
    let text =
        http_json(&workspace_summary::from_model(&sample_workspace(), &counts)).expect("dump");
    assert!(
        text.contains(r#""tasks_by_status":{"ready":3,"blocked":1}"#),
        "{text}"
    );
    assert!(text.contains(r#""ready_tasks":3"#), "{text}");
    assert!(text.contains(r#""queued_jobs":4"#), "{text}");
}

#[test]
fn project_summary_open_tasks_exclude_accepted_and_cancelled() {
    let statuses = [
        ("ready".to_string(), 2),
        ("accepted".to_string(), 3),
        ("cancelled".to_string(), 1),
        ("blocked".to_string(), 4),
    ];
    let text = http_json(&project_summary::from_model(
        Value::Null,
        &[],
        &statuses,
        Vec::new(),
    ))
    .expect("dump");
    assert!(text.contains(r#""open_tasks":6"#), "{text}");
    assert!(text.contains(r#""accepted_tasks":3"#), "{text}");
}

#[test]
fn repository_out_prints_nulls_and_checkouts() {
    let repo = sample_repository();
    let checkouts = [sample_checkout(&repo)];
    let text = http_json(&repository::from_model(&repo, &[], &checkouts, [], [])).expect("dump");
    assert!(text.contains(r#""identity_fingerprint":null"#), "{text}");
    assert!(text.contains(r#""default_branch":"main""#), "{text}");
    assert!(text.contains(r#""tracked_branches":[]"#), "{text}");
    assert!(text.contains(r#""project_ids":[]"#), "{text}");
    assert!(
        text.contains(
            r#""checkouts":[{"id":"00000000-0000-4000-8000-0000000000c1","path":"C:/dev/wg","kind":"clone","is_primary":true,"available":true,"access_mode":null,"last_seen_at":null}]"#
        ),
        "{text}"
    );
}

#[test]
fn repository_status_prints_unavailable_checkout() {
    let status = Status {
        error: Some("no available checkout".into()),
        ..Status::default()
    };
    let text = http_json(&repository_status::from_model(
        id_of("00000000-0000-4000-8000-0000000000a1"),
        "wg",
        &status,
    ))
    .expect("dump");
    assert_eq!(
        text,
        concat!(
            r#"{"repository_id":"00000000-0000-4000-8000-0000000000a1","name":"wg","#,
            r#""checkout_path":null,"available":false,"branch":null,"head":null,"detached":false,"#,
            r#""staged":[],"unstaged":[],"untracked":[],"ahead":null,"behind":null,"is_shallow":false,"#,
            r#""error":"no available checkout"}"#
        )
    );
}

#[test]
fn mcp_list_shapes_wrap_the_items() {
    let ws = sample_workspace();
    let items = vec![workspace::from_model(&ws)];
    let text = http_json(&mcp_shapes::workspaces(items)).expect("dump");
    assert!(
        text.starts_with(r#"{"workspaces":[{"id":"00000000-0000-4000-8000-000000000001""#),
        "{text}"
    );
    assert_eq!(
        http_json(&mcp_shapes::projects(Vec::new())).expect("dump"),
        r#"{"projects":[]}"#
    );
    assert_eq!(
        http_json(&mcp_shapes::repositories(Vec::new())).expect("dump"),
        r#"{"repositories":[]}"#
    );
}
