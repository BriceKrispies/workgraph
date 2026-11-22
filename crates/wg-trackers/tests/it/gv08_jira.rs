//! GV-08: every case of `golden/jira/{map_issue,adf_to_text,pr_url,status_mapping}.json` must
//! match the mapper, the ADF flattener, the pull request parser and the status policy.
//!
//! The golden files were recorded by `tools/golden/gen_jira.py` from the Python implementation.
//! Each case is rebuilt here in the same JSON shape and compared as a whole.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use wg_trackers::{
    MapError, MappingConfig, PullRequestRef, TrackerSnapshot, adf_to_text, map_issue,
    parse_pull_request_url, tracker_target_status,
};
use wg_types::TaskStatus;

fn golden(name: &str) -> Vec<Value> {
    let path = format!(
        "{}/../../golden/jira/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {path}: {err}"));
    let document: Value =
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("parsing {path}: {err}"));
    document["cases"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("{name} has no cases"))
}

/// The configuration of a golden case. Python's `dev_panel` defaults to true when absent.
fn config_of(value: &Value) -> MappingConfig {
    MappingConfig {
        base_url: value["base_url"].as_str().unwrap_or_default().to_string(),
        pr_fields: value["pr_fields"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        pr_default_repos: Default::default(),
        assignee_field: None,
        dev_panel: value["dev_panel"].as_bool().unwrap_or(true),
    }
}

fn ref_json(reference: &PullRequestRef) -> Value {
    json!({
        "url": reference.url,
        "normalized_url": reference.normalized_url,
        "provider": reference.provider,
        "repo_full_name": reference.repo_full_name,
        "repo_normalized_url": reference.repo_normalized_url,
        "number": reference.number,
        "sources": reference.sources,
        "title": reference.title,
    })
}

fn snapshot_json(snapshot: &TrackerSnapshot) -> Value {
    json!({
        "external_id": snapshot.external_id,
        "key": snapshot.key,
        "url": snapshot.url,
        "project_key": snapshot.project_key,
        "title": snapshot.title,
        "description": snapshot.description,
        "priority": snapshot.priority.as_str(),
        "assignee": snapshot.assignee,
        "status_name": snapshot.status_name,
        "status_category": snapshot.status_category.as_str(),
        "resolution": snapshot.resolution,
        "parent_key": snapshot.parent_key,
        "blocked_by_keys": snapshot.blocked_by_keys,
        "pull_requests": snapshot.pull_requests.iter().map(ref_json).collect::<Vec<_>>(),
        "remote_updated_at": snapshot.remote_updated_at.format_isoformat(),
        "issue_type": snapshot.issue_type,
        "labels": snapshot.labels,
        "raw": snapshot.raw,
    })
}

#[test]
fn map_issue_cases_match_the_golden_snapshots() {
    let cases = golden("map_issue");
    assert!(!cases.is_empty(), "map_issue has no cases");
    for case in &cases {
        let name = case["name"].as_str().unwrap_or_default();
        let dev: Vec<Value> = case["dev_pull_requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let config = config_of(&case["config"]);
        match map_issue(&case["issue"], &dev, &config) {
            Ok(snapshot) => {
                assert_eq!(snapshot_json(&snapshot), case["expected"], "case {name}");
            }
            Err(err) => {
                let expected = &case["error"];
                assert_eq!(expected["message"], err.message(), "case {name}");
                assert_eq!(expected["type"], "ValueError", "case {name}");
            }
        }
    }
}

#[test]
fn the_missing_timestamp_case_is_the_only_error_case() {
    let errors: Vec<Value> = golden("map_issue")
        .into_iter()
        .filter(|case| case.get("error").is_some())
        .map(|case| case["name"].clone())
        .collect();
    assert_eq!(errors, vec![json!("timestamp-missing-error")]);
    let missing = map_issue(
        &json!({"id": "1", "key": "ABC-1", "fields": {}}),
        &[],
        &MappingConfig {
            base_url: String::new(),
            pr_fields: Vec::new(),
            pr_default_repos: Default::default(),
            assignee_field: None,
            dev_panel: true,
        },
    );
    assert_eq!(missing.err(), Some(MapError::MissingUpdated));
}

#[test]
fn adf_to_text_cases_match_the_golden_text() {
    let cases = golden("adf_to_text");
    assert!(!cases.is_empty(), "adf_to_text has no cases");
    for case in &cases {
        let name = case["name"].as_str().unwrap_or_default();
        let input = case.get("input").cloned().unwrap_or(Value::Null);
        assert_eq!(json!(adf_to_text(&input)), case["expected"], "case {name}");
    }
}

#[test]
fn pr_url_cases_match_the_golden_references() {
    let cases = golden("pr_url");
    assert!(!cases.is_empty(), "pr_url has no cases");
    for case in &cases {
        let url = case["url"].as_str().unwrap_or_default();
        let parsed = parse_pull_request_url(url).map(|reference| ref_json(&reference));
        let expected = match &case["expected"] {
            Value::Null => None,
            value => Some(value.clone()),
        };
        assert_eq!(parsed, expected, "url {url:?}");
    }
}

#[test]
fn status_mapping_cases_match_the_golden_targets() {
    let cases = golden("status_mapping");
    assert!(
        cases.len() > 1000,
        "status_mapping should cover every combination"
    );
    for case in &cases {
        let current = TaskStatus::parse(case["current"].as_str().unwrap_or_default())
            .expect("golden status is a task status");
        let category = case["category"].as_str().unwrap_or_default();
        let resolution = case["resolution"].as_str();
        let cancelled: Vec<&str> = case["cancelled_resolutions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let target = tracker_target_status(current, category, resolution, &cancelled);
        assert_eq!(
            json!(target.map(TaskStatus::as_str)),
            case["expected"],
            "{current:?} / {category:?} / {resolution:?} / {cancelled:?}"
        );
    }
}
