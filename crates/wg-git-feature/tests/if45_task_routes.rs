//! The task commit and change routes (H047 to H051, IF-45): their table in `http`, the bodies they
//! name in the OpenAPI snapshot, and the shape of the change-set reply.

use rpds::VectorSync;
use serde_json::{Value as Json, json};
use wg_git_feature::http::SPECS;
use wg_git_feature::spec::reads;
use wg_model::entity::{ChangeSet, ChangeSetItem};
use wg_types::{ChangeSetItemStatus, ChangeSetStatus, Id, Timestamp, parse_id};

const WORKSPACE: &str = "018f0000-0000-7000-8000-000000000001";
const REPOSITORY: &str = "018f0000-0000-7000-8000-000000000002";
const TASK: &str = "018f0000-0000-7000-8000-000000000003";
const SET: &str = "018f0000-0000-7000-8000-000000000004";
const ITEM: &str = "018f0000-0000-7000-8000-000000000005";
const ATTEMPT: &str = "018f0000-0000-7000-8000-000000000006";

fn id(text: &str) -> Id {
    parse_id(text).unwrap_or_else(|err| panic!("a valid uuid: {err:?}"))
}

fn at() -> Timestamp {
    Timestamp::parse_iso("2026-10-26T10:00:00+00:00")
        .unwrap_or_else(|err| panic!("a valid timestamp: {err:?}"))
}

fn change_set(attempt: Option<Id>) -> ChangeSet {
    ChangeSet {
        id: id(SET),
        workspace_id: id(WORKSPACE),
        task_id: id(TASK),
        attempt_id: attempt,
        title: "Add the parser".to_owned(),
        status: ChangeSetStatus::Open,
        created_at: at(),
        updated_at: at(),
    }
}

fn item() -> ChangeSetItem {
    ChangeSetItem {
        id: id(ITEM),
        change_set_id: id(SET),
        repository_id: id(REPOSITORY),
        base_sha: Some("1111111".to_owned()),
        head_sha: None,
        branch: Some("task/WG-1".to_owned()),
        commit_shas: VectorSync::new_sync().push_back("abc123".to_owned()),
        status: ChangeSetItemStatus::Pending,
        detail: None,
        updated_at: at(),
    }
}

// covers: H047 H048 H049 H050 H051
#[test]
fn the_commit_and_change_routes_sit_in_the_git_table_at_their_paths() {
    let rows: Vec<(&str, &str, &str, u16)> = SPECS
        .iter()
        .map(|spec| (spec.id, spec.method, spec.path, spec.status))
        .collect();
    assert_eq!(
        rows,
        [
            ("H022", "GET", "/api/v1/ws/{ws}/worktrees", 200),
            ("H047", "GET", "/api/v1/ws/{ws}/tasks/{ref}/commits", 200),
            ("H048", "POST", "/api/v1/ws/{ws}/tasks/{ref}/commits", 200),
            ("H049", "GET", "/api/v1/ws/{ws}/tasks/{ref}/changes", 200),
            ("H050", "GET", "/api/v1/ws/{ws}/tasks/{ref}/diff", 200),
            (
                "H051",
                "GET",
                "/api/v1/ws/{ws}/tasks/{ref}/change-sets",
                200
            ),
        ]
    );
}

// covers: H047 H048 H049 H050 H051
#[test]
fn the_link_body_is_the_snapshot_model_and_is_not_stripped() {
    let openapi = wg_wire::contract::openapi().unwrap_or_default();
    for spec in SPECS.iter().filter(|spec| spec.id != "H022") {
        assert_eq!(
            wg_iface::http::operations::group_of(spec.id),
            "IF-45",
            "{}",
            spec.id
        );
        if let Some(body) = &spec.body {
            assert_eq!(body.component, "LinkCommitBody");
            assert!(
                !body.strip,
                "LinkCommitBody is a BaseModel, not an InputSchema"
            );
            assert!(body.required);
            assert!(wg_iface::http::groups::body_spec(&openapi, body).is_some());
        }
    }
}

// covers: H051
#[test]
fn the_change_sets_reply_lists_each_set_with_its_items() {
    let sets = vec![(change_set(Some(id(ATTEMPT))), vec![item()])];
    let value = serde_json::to_value(reads::change_sets_value(&sets)).expect("a JSON value");
    assert_eq!(
        value,
        json!([{
            "id": SET,
            "attempt_id": ATTEMPT,
            "title": "Add the parser",
            "status": "open",
            "created_at": "2026-10-26T10:00:00+00:00",
            "items": [{
                "repository_id": REPOSITORY,
                "base_sha": "1111111",
                "head_sha": Json::Null,
                "branch": "task/WG-1",
                "commits": ["abc123"],
                "status": "pending",
                "detail": Json::Null,
            }],
        }])
    );
}

// covers: H051
#[test]
fn a_change_set_without_an_attempt_or_items_keeps_its_nulls_and_empty_list() {
    let sets = vec![(change_set(None), Vec::new())];
    let value = serde_json::to_value(reads::change_sets_value(&sets)).expect("a JSON value");
    assert_eq!(value[0]["attempt_id"], Json::Null);
    assert_eq!(value[0]["items"], json!([]));
}

// covers: H047
#[test]
fn no_linked_commit_gives_an_empty_commit_list() {
    let value = serde_json::to_value(reads::task_commits_value(&[])).expect("a JSON value");
    assert_eq!(value, json!([]));
}
