//! IF-43 (registry part): the route tables of the project routes (H006 to H011) and the repository
//! routes (H012 to H021), and the body rules the handlers apply to their requests.

#![allow(clippy::expect_used)]

use serde_json::{Value, json};
use wg_git_feature::service::git_sync_ref;
use wg_git_feature::spec::mode::sync_is_full;
use wg_registry_feature::http::{PROJECT_SPECS, REPOSITORY_SPECS};
use wg_registry_feature::service::workspace_create;
use wg_registry_feature::spec::repos::{register_input, repository_patch};
use wg_runtime::{App, human_context};
use wg_serverspec::groups::RouteSpec;
use wg_types::{AccessMode, ErrorCode};

fn spec(specs: &[RouteSpec], id: &str) -> RouteSpec {
    *specs
        .iter()
        .find(|spec| spec.id == id)
        .expect("the operation is in its table")
}

// covers: H006
#[test]
fn project_list_is_a_get_with_the_archived_flag() {
    let listed = spec(&PROJECT_SPECS, "H006");
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, "/api/v1/ws/{ws}/projects");
    assert_eq!(listed.status, 200);
    assert_eq!(listed.query, ["include_archived"]);
    assert!(listed.body.is_none());
}

// covers: H007
#[test]
fn project_create_takes_a_stripped_project_create_body_and_answers_201() {
    let created = spec(&PROJECT_SPECS, "H007");
    assert_eq!(created.method, "POST");
    assert_eq!(created.status, 201);
    let body = created.body.expect("a body");
    assert_eq!(body.component, "ProjectCreate");
    assert!(body.required && body.strip);
}

// covers: H008
#[test]
fn project_summary_is_a_get_on_the_project_reference() {
    let summary = spec(&PROJECT_SPECS, "H008");
    assert_eq!(summary.method, "GET");
    assert_eq!(summary.path, "/api/v1/ws/{ws}/projects/{ref}");
    assert!(summary.body.is_none());
}

// covers: H009
#[test]
fn project_update_is_a_patch_with_a_stripped_project_update_body() {
    let update = spec(&PROJECT_SPECS, "H009");
    assert_eq!(update.method, "PATCH");
    let body = update.body.expect("a body");
    assert_eq!(body.component, "ProjectUpdate");
    assert!(body.required && body.strip);
}

// covers: H010
#[test]
fn project_add_repo_takes_a_repo_link_body_that_keeps_its_whitespace() {
    let link = spec(&PROJECT_SPECS, "H010");
    assert_eq!(link.method, "POST");
    assert_eq!(link.path, "/api/v1/ws/{ws}/projects/{ref}/repositories");
    assert_eq!(link.status, 200);
    let body = link.body.expect("a body");
    assert_eq!(body.component, "RepoLink");
    assert!(body.required && !body.strip);
}

// covers: H011
#[test]
fn project_remove_repo_is_a_delete_on_the_repository_of_the_project() {
    let unlink = spec(&PROJECT_SPECS, "H011");
    assert_eq!(unlink.method, "DELETE");
    assert_eq!(
        unlink.path,
        "/api/v1/ws/{ws}/projects/{ref}/repositories/{repository}"
    );
    assert!(unlink.body.is_none());
}

// covers: H012
#[test]
fn repository_list_reads_the_archived_flag_and_the_project_filter() {
    let listed = spec(&REPOSITORY_SPECS, "H012");
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.query, ["include_archived", "project"]);
    assert!(listed.body.is_none());
}

// covers: H013
#[test]
fn repository_register_answers_201_with_a_stripped_body() {
    let register = spec(&REPOSITORY_SPECS, "H013");
    assert_eq!(register.method, "POST");
    assert_eq!(register.status, 201);
    let body = register.body.expect("a body");
    assert_eq!(body.component, "RepositoryRegister");
    assert!(body.required && body.strip);
}

// covers: H014
#[test]
fn repository_get_is_a_get_on_the_repository_reference() {
    let get = spec(&REPOSITORY_SPECS, "H014");
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/api/v1/ws/{ws}/repositories/{ref}");
}

// covers: H015
#[test]
fn repository_update_is_a_patch_with_a_stripped_body() {
    let update = spec(&REPOSITORY_SPECS, "H015");
    assert_eq!(update.method, "PATCH");
    let body = update.body.expect("a body");
    assert_eq!(body.component, "RepositoryUpdate");
    assert!(body.required && body.strip);
}

// covers: H016
#[test]
fn repository_remove_is_a_delete_without_a_body() {
    let remove = spec(&REPOSITORY_SPECS, "H016");
    assert_eq!(remove.method, "DELETE");
    assert!(remove.body.is_none());
}

// covers: H017
#[test]
fn repository_refresh_is_a_post_on_the_refresh_path() {
    let refresh = spec(&REPOSITORY_SPECS, "H017");
    assert_eq!(refresh.method, "POST");
    assert_eq!(refresh.path, "/api/v1/ws/{ws}/repositories/{ref}/refresh");
}

// covers: H018
#[test]
fn repository_status_is_a_get_on_the_status_path() {
    let status = spec(&REPOSITORY_SPECS, "H018");
    assert_eq!(status.method, "GET");
    assert_eq!(status.path, "/api/v1/ws/{ws}/repositories/{ref}/status");
}

// covers: H019
#[test]
fn repository_sync_is_a_post_that_reads_the_mode() {
    let sync = spec(&REPOSITORY_SPECS, "H019");
    assert_eq!(sync.method, "POST");
    assert_eq!(sync.query, ["mode"]);
    assert!(sync.body.is_none());
}

// covers: H020
#[test]
fn repository_commits_reads_the_page_and_the_two_filters() {
    let commits = spec(&REPOSITORY_SPECS, "H020");
    assert_eq!(commits.method, "GET");
    assert_eq!(
        commits.query,
        ["limit", "offset", "linked_only", "include_unreachable"]
    );
}

// covers: H021
#[test]
fn commit_detail_is_a_get_on_the_sha_path() {
    let detail = spec(&REPOSITORY_SPECS, "H021");
    assert_eq!(detail.method, "GET");
    assert_eq!(
        detail.path,
        "/api/v1/ws/{ws}/repositories/{ref}/commits/{sha}"
    );
}

#[test]
fn the_tables_name_their_operations_in_order_and_in_the_if_43_group() {
    let projects: Vec<&str> = PROJECT_SPECS.iter().map(|spec| spec.id).collect();
    assert_eq!(projects, ["H006", "H007", "H008", "H009", "H010", "H011"]);
    let repositories: Vec<&str> = REPOSITORY_SPECS.iter().map(|spec| spec.id).collect();
    assert_eq!(
        repositories,
        [
            "H012", "H013", "H014", "H015", "H016", "H017", "H018", "H019", "H020", "H021"
        ]
    );
    for spec in PROJECT_SPECS.iter().chain(REPOSITORY_SPECS.iter()) {
        assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-43");
    }
}

// covers: H013
#[test]
fn register_input_reads_the_body_fields_and_the_read_only_access_mode() {
    let body = json!({
        "path": "/work/repo",
        "name": "repo",
        "description": "",
        "access_mode": "read_only",
        "tracked_branches": ["main", "dev"],
        "projects": ["alpha"],
    });
    let input = register_input(&body);
    assert_eq!(input.path, "/work/repo");
    assert_eq!(input.name.as_deref(), Some("repo"));
    assert!(input.read_only);
    assert_eq!(input.tracked_branches, ["main", "dev"]);
    assert_eq!(input.projects, ["alpha"]);
}

// covers: H013
#[test]
fn register_input_defaults_to_a_read_write_repository_without_a_name() {
    let body = json!({"path": "/work/repo", "name": null, "access_mode": "read_write"});
    let input = register_input(&body);
    assert!(input.name.is_none());
    assert!(!input.read_only);
    assert!(input.tracked_branches.is_empty());
    assert!(input.projects.is_empty());
}

// covers: H015
#[test]
fn repository_patch_sets_only_the_fields_the_body_names() {
    let body = json!({
        "name": null,
        "description": "docs",
        "access_mode": "read_write",
        "default_branch": null,
        "tracked_branches": ["main"],
    });
    let patch = repository_patch(&body).expect("a valid patch");
    assert_eq!(patch.name, None);
    assert_eq!(patch.description.as_deref(), Some("docs"));
    assert_eq!(patch.access_mode, Some(AccessMode::ReadWrite));
    assert_eq!(patch.default_branch, None);
    assert_eq!(patch.tracked_branches, Some(vec!["main".to_owned()]));
}

// covers: H015
#[test]
fn repository_patch_leaves_every_field_alone_for_an_empty_body() {
    let patch = repository_patch(&Value::Object(serde_json::Map::new())).expect("a patch");
    assert_eq!(patch, wg_registry::admin::Patch::default());
}

// covers: H015
#[test]
fn repository_patch_refuses_an_unknown_access_mode() {
    let err = repository_patch(&json!({"access_mode": "write_only"})).expect_err("refused");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
}

// covers: H019
#[test]
fn sync_mode_is_incremental_or_full_and_nothing_else() {
    assert!(!sync_is_full("incremental").expect("mode"));
    assert!(sync_is_full("full").expect("mode"));
    let err = sync_is_full("quick").expect_err("refused");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(err.message, "mode must be incremental or full");
}

// covers: H019
#[test]
fn sync_resolves_the_repository_before_it_checks_the_mode() {
    let root = std::env::temp_dir().join(format!(
        "wg-if43-sync-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let mut app = App::open(Some(&root.join("home"))).expect("open the home");
    let context = human_context(None).expect("a context");
    workspace_create(
        &mut app,
        &context,
        &json!({"slug": "demo", "name": "Demo"}),
        false,
    )
    .expect("the workspace");
    // Both wrong: the repository does not exist and the mode is not one the service knows. Python
    // resolves the repository first, so the answer is the not-found error, not the mode error.
    let err = git_sync_ref(&mut app, &context, Some("demo"), "no-such-repo", "sideways")
        .expect_err("refused");
    assert_eq!(err.code, ErrorCode::NotFound);
    let _ = std::fs::remove_dir_all(&root);
}

// covers: H007
#[test]
fn every_body_of_the_tables_loads_from_the_snapshot() {
    let openapi = wg_wire::contract::openapi().expect("the snapshot");
    for spec in PROJECT_SPECS.iter().chain(REPOSITORY_SPECS.iter()) {
        if let Some(body) = &spec.body {
            assert!(
                wg_serverspec::groups::body_spec(&openapi, body).is_some(),
                "{}",
                spec.id
            );
        }
    }
}
