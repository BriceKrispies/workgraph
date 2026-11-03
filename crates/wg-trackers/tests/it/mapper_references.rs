#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Pull request shorthand in PR fields, `pr_default_repos`, and `assignee_field`.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use wg_trackers::{MappingConfig, map_issue};

const FIELD: &str = "customfield_10042";

fn config(defaults: &[(&str, &str)], assignee_field: Option<&str>) -> MappingConfig {
    MappingConfig {
        base_url: "https://acme.atlassian.net".to_string(),
        pr_fields: vec![FIELD.to_string()],
        pr_default_repos: defaults
            .iter()
            .map(|(field, url)| ((*field).to_string(), (*url).to_string()))
            .collect::<BTreeMap<_, _>>(),
        assignee_field: assignee_field.map(str::to_string),
        dev_panel: false,
    }
}

fn issue(extra: Value) -> Value {
    let mut fields = json!({
        "summary": "s",
        "project": {"key": "ABC"},
        "updated": "2026-10-18T09:15:02.123+0000",
    });
    for (key, value) in extra.as_object().unwrap() {
        fields[key] = value.clone();
    }
    json!({"id": "1", "key": "ABC-1", "fields": fields})
}

fn urls_with(value: Value, defaults: &[(&str, &str)]) -> Vec<String> {
    let mapped = map_issue(
        &issue(json!({ FIELD: value })),
        &[],
        &config(defaults, None),
    )
    .expect("maps");
    mapped
        .pull_requests
        .iter()
        .map(|reference| reference.normalized_url.clone())
        .collect()
}

fn urls(text: &str) -> Vec<String> {
    urls_with(json!(text), &[])
}

const SVC: &[(&str, &str)] = &[(FIELD, "https://github.com/acme/svc")];

#[test]
fn shorthand_and_continuations_take_the_last_urls_repository() {
    let cases: &[(&str, &[&str])] = &[
        (
            "github.com/acme/svc/pull/5",
            &["github.com/acme/svc/pull/5"],
        ),
        (
            "www.github.com/acme/svc/pull/5/files",
            &["github.com/acme/svc/pull/5"],
        ),
        (
            "https://github.com/acme/svc/pull/5 + /pull/7",
            &["github.com/acme/svc/pull/5", "github.com/acme/svc/pull/7"],
        ),
        (
            "https://github.com/acme/svc/pull/5, 6 and #8",
            &[
                "github.com/acme/svc/pull/5",
                "github.com/acme/svc/pull/6",
                "github.com/acme/svc/pull/8",
            ],
        ),
        (
            "https://github.com/acme/a/pull/1 then https://github.com/acme/b/pull/2 + pull/3",
            &[
                "github.com/acme/a/pull/1",
                "github.com/acme/b/pull/2",
                "github.com/acme/b/pull/3",
            ],
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(urls(text), *expected, "text {text:?}");
    }
}

#[test]
fn without_a_repository_shorthand_is_skipped_not_guessed() {
    for text in [
        "pull/7",
        "1749, 1809",
        "https://github.com/acme/svc/issues/9 and pull/3",
        "released 2026-10-01, see ABC-1234",
        "see github.com/acme/svc/pull/x",
    ] {
        assert!(urls(text).is_empty(), "text {text:?}");
    }
}

#[test]
fn bare_numbers_use_the_fields_default_repository() {
    assert_eq!(
        urls_with(json!("1749, #1809 / 1822"), SVC),
        [
            "github.com/acme/svc/pull/1749",
            "github.com/acme/svc/pull/1809",
            "github.com/acme/svc/pull/1822",
        ]
    );
    assert_eq!(
        urls_with(json!(1749), SVC),
        ["github.com/acme/svc/pull/1749"]
    );
}

#[test]
fn shorthand_without_a_url_uses_the_default_repository() {
    let trailing_slash = &[(FIELD, "https://github.com/acme/svc/")];
    assert_eq!(
        urls_with(json!("PR: pull/42"), trailing_slash),
        ["github.com/acme/svc/pull/42"]
    );
}

#[test]
fn numbers_inside_prose_are_not_pull_requests_even_with_a_default() {
    assert!(urls_with(json!("deployed on 2026-10-01 after 3 tries"), SVC).is_empty());
}

#[test]
fn a_url_overrides_the_default_repository_for_its_shorthand() {
    assert_eq!(
        urls_with(
            json!("https://github.com/acme/other/pull/1 and pull/2"),
            SVC
        ),
        [
            "github.com/acme/other/pull/1",
            "github.com/acme/other/pull/2"
        ]
    );
}

#[test]
fn adf_link_targets_and_shorthand_after_them_are_read() {
    let value = json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [
                {
                    "type": "text",
                    "text": "the fix",
                    "marks": [{"type": "link", "attrs": {"href": "https://github.com/acme/svc/pull/5"}}],
                },
                {"type": "text", "text": " + /pull/6"},
            ],
        }],
    });
    assert_eq!(
        urls_with(value, &[]),
        ["github.com/acme/svc/pull/5", "github.com/acme/svc/pull/6"]
    );
}

#[test]
fn long_digit_runs_are_not_pull_requests() {
    let text = "1".repeat(50_000);
    assert!(urls_with(json!(text), SVC).is_empty());
    let words = "a".repeat(50_000);
    assert!(urls(&words).is_empty());
}

#[test]
fn assignee_field_reads_a_custom_user_field() {
    let mapped = map_issue(
        &issue(json!({
            "assignee": {"displayName": "Reviewer"},
            "customfield_20000": {"displayName": "Builder"},
        })),
        &[],
        &config(&[], Some("customfield_20000")),
    )
    .expect("maps");
    assert_eq!(mapped.assignee.as_deref(), Some("Builder"));
}

#[test]
fn assignee_field_takes_the_first_user_of_a_multi_user_field() {
    let mapped = map_issue(
        &issue(json!({
            "customfield_20000": [null, {"displayName": "First"}, {"displayName": "Second"}],
        })),
        &[],
        &config(&[], Some("customfield_20000")),
    )
    .expect("maps");
    assert_eq!(mapped.assignee.as_deref(), Some("First"));
}

#[test]
fn an_empty_assignee_field_is_none_without_falling_back() {
    let mapped = map_issue(
        &issue(json!({
            "assignee": {"displayName": "Reviewer"},
            "customfield_20000": null,
        })),
        &[],
        &config(&[], Some("customfield_20000")),
    )
    .expect("maps");
    assert_eq!(mapped.assignee, None);
}
