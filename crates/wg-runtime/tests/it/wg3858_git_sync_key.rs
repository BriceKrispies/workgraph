//! WG-3858: a `git_sync` job enqueued from the CLI or the post-commit hook takes its repository's
//! resource key, `repo:<id>`, as the registration sync does, so the two serialise.

use serde_json::{Value, json};
use wg_runtime::spec::jobs::default_resource_keys;

fn payload(value: Value) -> wg_model::entity::JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn git_sync_takes_its_repository_key() {
    let got = default_resource_keys(
        "git_sync",
        &payload(json!({"repository_id": "repo-1", "mode": "incremental"})),
    );
    assert_eq!(got, vec!["repo:repo-1".to_string()]);
}

#[test]
fn git_sync_without_a_repository_takes_no_key() {
    assert!(default_resource_keys("git_sync", &payload(json!({}))).is_empty());
    assert!(default_resource_keys("git_sync", &payload(json!({"repository_id": 7}))).is_empty());
}

#[test]
fn other_kinds_keep_their_empty_keys() {
    let body = payload(json!({"repository_id": "repo-1"}));
    assert!(default_resource_keys("analyze_commits", &body).is_empty());
    assert!(default_resource_keys("noop", &body).is_empty());
}
