#![allow(clippy::expect_used, clippy::unwrap_used)]
//! The Jira connection's credential, scope and pull request rules.

use serde_json::{Value, json};
use wg_config::validate_global;

fn tracker(extra: Value) -> Value {
    let mut tracker = json!({
        "type": "jira",
        "base_url": "https://example.atlassian.net",
        "workspace": "work",
        "projects": ["ABC"],
    });
    for (key, value) in extra.as_object().unwrap() {
        tracker[key] = value.clone();
    }
    json!({"trackers": {"jira": tracker}})
}

/// The messages of the validation errors, or empty when the document is valid.
fn errors(extra: Value) -> Vec<String> {
    match validate_global(&tracker(extra)) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|error| error.msg).collect(),
    }
}

#[test]
fn a_token_command_with_bearer_auth_is_valid_and_dumps_its_settings() {
    let dump = validate_global(&tracker(json!({
        "base_url": "https://api.atlassian.com/ex/jira/00000000-0000-0000-0000-000000000000",
        "site_url": "https://example.atlassian.net",
        "auth": "bearer",
        "token_command": ["oauth-helper", "token"],
        "jql": "assignee = currentUser()",
        "min_request_interval_seconds": 0.5,
    })))
    .expect("valid");
    let jira = &dump["trackers"]["jira"];
    assert_eq!(jira["token_env"], Value::Null);
    assert_eq!(jira["token_command"], json!(["oauth-helper", "token"]));
    assert_eq!(jira["token_command_timeout_seconds"], json!(60.0));
    assert_eq!(jira["pr_default_repos"], json!({}));
    assert_eq!(jira["assignee_field"], Value::Null);
}

#[test]
fn exactly_one_credential_source_is_required() {
    let rule = "Value error, set exactly one of token_env and token_command";
    assert_eq!(errors(json!({})), [rule]);
    assert_eq!(
        errors(json!({"token_env": "T", "token_command": ["helper"]})),
        [rule]
    );
    assert!(errors(json!({"token_env": "T"})).is_empty());
}

#[test]
fn a_token_command_needs_an_executable() {
    for argv in [json!([]), json!(["  ", "token"])] {
        assert_eq!(
            errors(json!({"token_command": argv})),
            ["Value error, token_command needs at least the executable"]
        );
    }
}

#[test]
fn email_env_is_only_for_basic_auth() {
    assert_eq!(
        errors(json!({"auth": "bearer", "email_env": "E", "token_env": "T"})),
        ["Value error, email_env is only used with auth = 'basic'"]
    );
    assert!(errors(json!({"auth": "basic", "email_env": "E", "token_env": "T"})).is_empty());
}

#[test]
fn jql_may_not_order_the_search() {
    assert_eq!(
        errors(json!({"token_env": "T", "jql": "labels = x order  BY created"})).len(),
        0,
        "only the exact phrase is refused"
    );
    assert_eq!(
        errors(json!({"token_env": "T", "jql": "labels = x ORDER BY created"})),
        ["Value error, jql must not contain ORDER BY; Workgraph orders the search itself"]
    );
}

#[test]
fn the_request_interval_is_bounded() {
    assert!(errors(json!({"token_env": "T", "min_request_interval_seconds": 60})).is_empty());
    assert_eq!(
        errors(json!({"token_env": "T", "min_request_interval_seconds": 61})),
        ["Value error, must be at most 60 seconds"]
    );
    assert_eq!(
        errors(json!({"token_env": "T", "min_request_interval_seconds": -1})).len(),
        1
    );
}

#[test]
fn pr_default_repos_must_name_a_pr_field_and_a_repository() {
    let base = json!({"token_env": "T", "pr_fields": ["customfield_10042"]});
    let with = |defaults: Value| {
        let mut extra = base.clone();
        extra["pr_default_repos"] = defaults;
        errors(extra)
    };
    assert!(with(json!({"customfield_10042": "https://github.com/acme/svc"})).is_empty());
    assert_eq!(
        with(json!({"customfield_99999": "https://github.com/acme/svc"})),
        ["Value error, pr_default_repos names 'customfield_99999', which is not in pr_fields"]
    );
    for url in [
        "github.com/acme/svc",
        "https://github.com/acme",
        "https://github.com/acme/svc/pull/1",
        "https://localhost/acme/svc",
    ] {
        assert_eq!(
            with(json!({"customfield_10042": url})).len(),
            1,
            "url {url:?}"
        );
    }
}

#[test]
fn worktree_root_is_an_optional_execution_setting() {
    let dump = validate_global(&json!({"execution": {"worktree_root": "D:/wt"}})).expect("valid");
    assert_eq!(dump["execution"]["worktree_root"], json!("D:/wt"));
    let default = validate_global(&json!({})).expect("valid");
    assert_eq!(default["execution"]["worktree_root"], Value::Null);
}
