//! CORE-23: the cases of `tests/unit/test_tracker_mapper.py`, ported. Jira issue JSON to a
//! tracker snapshot, the ADF flattening, and pull request URL parsing. No database, no network.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use wg_trackers::{
    MapError, MappingConfig, PullRequestRef, StatusCategory, TrackerSnapshot, adf_to_text,
    map_issue, parse_pull_request_url,
};
use wg_types::{TaskPriority, Timestamp};

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/fixtures/jira/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {path}: {err}"));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parsing {path}: {err}"))
}

fn config() -> MappingConfig {
    MappingConfig {
        base_url: "https://acme.atlassian.net".to_string(),
        pr_fields: vec!["customfield_10042".to_string()],
        dev_panel: true,
    }
}

fn with_config(base_url: &str, pr_fields: &[&str], dev_panel: bool) -> MappingConfig {
    MappingConfig {
        base_url: base_url.to_string(),
        pr_fields: pr_fields.iter().map(|field| field.to_string()).collect(),
        dev_panel,
    }
}

fn issue(fields: Value) -> Value {
    let mut base = json!({
        "summary": "Minimal issue",
        "project": {"key": "ABC"},
        "updated": "2026-10-18T09:15:02.123+0000",
    });
    let object = base.as_object_mut().expect("object");
    for (name, value) in fields.as_object().expect("object") {
        object.insert(name.clone(), value.clone());
    }
    json!({"id": "1", "key": "ABC-1", "fields": base})
}

fn pr_entry(url: &str, name: &str) -> Value {
    json!({"url": url, "name": name, "status": "OPEN"})
}

fn map(issue: &Value, dev: &[Value], config: &MappingConfig) -> TrackerSnapshot {
    map_issue(issue, dev, config).expect("issue maps")
}

fn utc(text: &str) -> Timestamp {
    Timestamp::parse_iso(text).expect("timestamp")
}

fn normalized(snapshot: &TrackerSnapshot) -> Vec<&str> {
    snapshot
        .pull_requests
        .iter()
        .map(|reference| reference.normalized_url.as_str())
        .collect()
}

fn story() -> TrackerSnapshot {
    let dev = fixture("dev_status.json")["detail"][0]["pullRequests"]
        .as_array()
        .cloned()
        .expect("dev pull requests");
    map(&fixture("cloud_story.json"), &dev, &config())
}

// --- cloud story: the full mapping ----------------------------------------------------------------

#[test]
fn scalar_fields_come_from_the_issue() {
    let story = story();
    assert_eq!(story.external_id, "10001");
    assert_eq!(story.key, "ABC-123");
    assert_eq!(story.url, "https://acme.atlassian.net/browse/ABC-123");
    assert_eq!(story.project_key, "ABC");
    assert_eq!(story.title, "Add retry budget to the billing client");
    assert_eq!(story.priority, TaskPriority::High);
    assert_eq!(story.assignee.as_deref(), Some("Brice Lehew"));
    assert_eq!(story.status_name, "In Progress");
    assert_eq!(story.status_category, StatusCategory::Indeterminate);
    assert_eq!(story.resolution, None);
    assert_eq!(story.parent_key.as_deref(), Some("ABC-100"));
    assert_eq!(story.issue_type, "Story");
    assert_eq!(story.labels, vec!["billing", "retries"]);
    assert_eq!(story.raw["key"], "ABC-123");
}

#[test]
fn remote_updated_at_is_utc() {
    assert_eq!(story().remote_updated_at, utc("2026-10-18T09:15:02.123Z"));
}

#[test]
fn adf_description_is_flattened() {
    assert_eq!(
        story().description,
        "Calls to the billing API fail intermittently. Bounded retries are the fix.\n\n\
         - Cap attempts at 3\n- Back off exponentially\n\n\
         Line one\nOwner: @Priya Rao see https://acme.atlassian.net/wiki/spaces/ENG/pages/1"
    );
}

#[test]
fn blocked_by_keys_come_from_inward_blocks_links_only() {
    // ABC-122 and ABC-118 (listed twice) are "is blocked by"; ABC-124 is an outward "blocks";
    // ABC-130 is a "Relates" link.
    assert_eq!(story().blocked_by_keys, vec!["ABC-118", "ABC-122"]);
}

#[test]
fn pull_requests_merge_custom_field_and_dev_panel() {
    let story = story();
    assert_eq!(
        normalized(&story),
        vec![
            "github.com/acme/svc-api/pull/43",
            "github.com/acme/svc-billing/pull/42",
            "github.com/acme/svc-web/pull/7",
        ]
    );
    let by_url = |url: &str| -> PullRequestRef {
        story
            .pull_requests
            .iter()
            .find(|reference| reference.normalized_url == url)
            .cloned()
            .expect("reference present")
    };

    let duplicated = by_url("github.com/acme/svc-billing/pull/42");
    assert_eq!(duplicated.sources, vec!["custom_field", "dev_panel"]);
    assert_eq!(duplicated.title, "ABC-123 Add retry budget");
    assert_eq!(
        duplicated.url,
        "https://github.com/acme/svc-billing/pull/42"
    );
    assert_eq!(duplicated.repo_full_name, "acme/svc-billing");
    assert_eq!(
        duplicated.repo_normalized_url,
        "github.com/acme/svc-billing"
    );
    assert_eq!(duplicated.number, 42);
    assert_eq!(duplicated.provider, "github");

    let dev_only = by_url("github.com/acme/svc-api/pull/43");
    assert_eq!(dev_only.sources, vec!["dev_panel"]);
    assert_eq!(dev_only.title, "ABC-123 Retry budget config");

    let custom_only = by_url("github.com/acme/svc-web/pull/7");
    assert_eq!(custom_only.sources, vec!["custom_field"]);
    assert_eq!(custom_only.title, "");
    assert_eq!(custom_only.repo_full_name, "Acme/Svc-Web");
    assert_eq!(custom_only.repo_normalized_url, "github.com/acme/svc-web");
}

#[test]
fn dev_panel_is_ignored_when_disabled() {
    let dev = fixture("dev_status.json")["detail"][0]["pullRequests"]
        .as_array()
        .cloned()
        .expect("dev pull requests");
    let mapped = map(
        &fixture("cloud_story.json"),
        &dev,
        &with_config("https://acme.atlassian.net", &["customfield_10042"], false),
    );
    assert_eq!(
        normalized(&mapped),
        vec![
            "github.com/acme/svc-billing/pull/42",
            "github.com/acme/svc-web/pull/7",
        ]
    );
    assert_eq!(mapped.pull_requests[0].sources, vec!["custom_field"]);
}

// --- other issues ----------------------------------------------------------------------------------

#[test]
fn done_issue_with_wont_do_resolution() {
    let mapped = map(&fixture("cloud_done_wont_do.json"), &[], &config());
    assert_eq!(mapped.status_category, StatusCategory::Done);
    assert_eq!(mapped.resolution.as_deref(), Some("Won't Do"));
    assert_eq!(mapped.priority, TaskPriority::Low);
    assert_eq!(mapped.description, "");
    assert_eq!(mapped.assignee, None);
    assert_eq!(mapped.parent_key, None);
    assert!(mapped.blocked_by_keys.is_empty());
    assert!(mapped.pull_requests.is_empty());
    assert_eq!(mapped.remote_updated_at, utc("2026-10-17T16:40:00Z"));
}

#[test]
fn data_center_issue_passes_wiki_markup_and_string_pr_list_through() {
    let mapped = map(
        &fixture("datacenter_issue.json"),
        &[],
        &with_config("https://jira.corp.example/", &["customfield_11000"], false),
    );
    assert_eq!(mapped.url, "https://jira.corp.example/browse/OPS-7");
    assert_eq!(mapped.project_key, "OPS");
    assert_eq!(
        mapped.description,
        "h2. Goal\n\nRotate the *signing key* without downtime.\n\n* step one\n* step two"
    );
    assert_eq!(mapped.priority, TaskPriority::High);
    assert_eq!(mapped.status_category, StatusCategory::New);
    assert_eq!(mapped.blocked_by_keys, vec!["OPS-3"]);
    assert_eq!(
        normalized(&mapped),
        vec![
            "github.com/acme/svc-auth/pull/10",
            "github.com/acme/svc-auth/pull/9",
        ]
    );
    assert!(
        mapped
            .pull_requests
            .iter()
            .all(|reference| reference.sources == vec!["custom_field"])
    );
}

// --- field rules -----------------------------------------------------------------------------------

#[test]
fn priority_mapping() {
    let cases: [(Option<&str>, TaskPriority); 14] = [
        (Some("Highest"), TaskPriority::Critical),
        (Some("Blocker"), TaskPriority::Critical),
        (Some("Critical"), TaskPriority::Critical),
        (Some("High"), TaskPriority::High),
        (Some("Major"), TaskPriority::High),
        (Some("Medium"), TaskPriority::Medium),
        (Some("Low"), TaskPriority::Low),
        (Some("Lowest"), TaskPriority::Low),
        (Some("Minor"), TaskPriority::Low),
        (Some("Trivial"), TaskPriority::Low),
        (Some("HIGH"), TaskPriority::High),
        (Some("  minor "), TaskPriority::Low),
        (Some("Showstopper"), TaskPriority::Medium),
        (None, TaskPriority::Medium),
    ];
    for (jira_name, expected) in cases {
        let fields = match jira_name {
            Some(name) => json!({"priority": {"name": name}}),
            None => json!({"priority": null}),
        };
        assert_eq!(
            map(&issue(fields), &[], &config()).priority,
            expected,
            "{jira_name:?}"
        );
    }
}

#[test]
fn missing_priority_field_is_medium() {
    assert_eq!(
        map(&issue(json!({})), &[], &config()).priority,
        TaskPriority::Medium
    );
}

#[test]
fn status_category_keys() {
    for (category, expected) in [
        ("new", StatusCategory::New),
        ("indeterminate", StatusCategory::Indeterminate),
        ("done", StatusCategory::Done),
        ("DONE", StatusCategory::Done),
    ] {
        let fields = json!({"status": {"name": "Whatever", "statusCategory": {"key": category}}});
        assert_eq!(
            map(&issue(fields), &[], &config()).status_category,
            expected
        );
    }
}

#[test]
fn unknown_status_category_is_new() {
    for status in [
        json!({"name": "Mystery", "statusCategory": {"key": "undefined"}}),
        json!({"name": "Mystery", "statusCategory": {}}),
        json!({"name": "Mystery"}),
    ] {
        let mapped = map(&issue(json!({"status": status})), &[], &config());
        assert_eq!(mapped.status_category, StatusCategory::New);
        assert_eq!(mapped.status_name, "Mystery");
    }
}

#[test]
fn missing_optional_fields_are_empty() {
    let mapped = map(&issue(json!({})), &[], &config());
    assert_eq!(mapped.title, "Minimal issue");
    assert_eq!(mapped.description, "");
    assert_eq!(mapped.assignee, None);
    assert_eq!(mapped.resolution, None);
    assert_eq!(mapped.parent_key, None);
    assert!(mapped.blocked_by_keys.is_empty());
    assert_eq!(mapped.issue_type, "");
    assert!(mapped.labels.is_empty());
    assert_eq!(mapped.status_name, "");
}

#[test]
fn assignee_without_display_name_is_none() {
    let fields = json!({"assignee": {"accountId": "x", "displayName": ""}});
    assert_eq!(map(&issue(fields), &[], &config()).assignee, None);
}

#[test]
fn project_key_falls_back_to_key_prefix() {
    let mut value = issue(json!({}));
    value["fields"]
        .as_object_mut()
        .expect("fields")
        .remove("project");
    assert_eq!(map(&value, &[], &config()).project_key, "ABC");
}

#[test]
fn base_url_trailing_slash_is_not_doubled() {
    let mapped = map(
        &issue(json!({})),
        &[],
        &with_config("https://acme.atlassian.net/", &["customfield_10042"], true),
    );
    assert_eq!(mapped.url, "https://acme.atlassian.net/browse/ABC-1");
}

#[test]
fn blocked_by_is_sorted_and_deduplicated() {
    let links = json!([
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-2"}},
        {"type": {"name": "Blocks"}, "inwardIssue": {"key": "ABC-9"}},
        {"type": {"name": "Blocks"}, "outwardIssue": {"key": "ABC-5"}},
        {"type": {"name": "Duplicate"}, "inwardIssue": {"key": "ABC-7"}},
    ]);
    let mapped = map(&issue(json!({"issuelinks": links})), &[], &config());
    assert_eq!(mapped.blocked_by_keys, vec!["ABC-2", "ABC-9"]);
}

#[test]
fn parent_key_is_none_without_parent() {
    assert_eq!(
        map(&issue(json!({"parent": null})), &[], &config()).parent_key,
        None
    );
}

#[test]
fn remote_timestamp_with_offset_is_converted_to_utc() {
    let mapped = map(
        &issue(json!({"updated": "2026-10-18T11:15:02.500+0200"})),
        &[],
        &config(),
    );
    assert_eq!(mapped.remote_updated_at, utc("2026-10-18T09:15:02.500Z"));
}

#[test]
fn missing_timestamp_is_an_error() {
    let mut value = issue(json!({}));
    value["fields"]
        .as_object_mut()
        .expect("fields")
        .remove("updated");
    let err = map_issue(&value, &[], &config()).expect_err("no timestamp");
    assert_eq!(err, MapError::MissingUpdated);
    assert!(err.message().contains("updated"));
}

// --- pull requests from custom fields --------------------------------------------------------------

#[test]
fn custom_field_without_pull_requests_gives_none() {
    for value in [
        Value::Null,
        json!(""),
        json!("no links here"),
        json!({"type": "doc", "version": 1, "content": []}),
        json!(["https://github.com/acme/svc-billing/issues/9"]),
    ] {
        let mapped = map(&issue(json!({"customfield_10042": value})), &[], &config());
        assert!(mapped.pull_requests.is_empty(), "{value}");
    }
}

#[test]
fn custom_field_shapes_all_yield_the_pull_request() {
    for value in [
        json!("https://github.com/acme/svc-api/pull/5"),
        json!(["https://github.com/acme/svc-api/pull/5 and more text"]),
        json!({"url": "https://github.com/acme/svc-api/pull/5", "value": "ignored"}),
        json!({
            "type": "doc",
            "content": [{"type": "inlineCard", "attrs": {"url": "https://github.com/acme/svc-api/pull/5"}}]
        }),
        json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "text",
                    "text": "svc-api",
                    "marks": [{"type": "link", "attrs": {"href": "https://github.com/acme/svc-api/pull/5"}}]
                }]
            }]
        }),
    ] {
        let mapped = map(&issue(json!({"customfield_10042": value})), &[], &config());
        assert_eq!(normalized(&mapped), vec!["github.com/acme/svc-api/pull/5"]);
        assert_eq!(mapped.pull_requests[0].sources, vec!["custom_field"]);
    }
}

#[test]
fn trailing_punctuation_is_not_part_of_the_url() {
    let fields = json!({"customfield_10042": "See https://github.com/acme/svc-api/pull/5."});
    let mapped = map(&issue(fields), &[], &config());
    assert_eq!(
        mapped.pull_requests[0].url,
        "https://github.com/acme/svc-api/pull/5"
    );
}

#[test]
fn several_pr_fields_are_all_read() {
    let config = with_config(
        "https://acme.atlassian.net",
        &["customfield_10042", "customfield_10043"],
        true,
    );
    let fields = json!({
        "customfield_10042": "https://github.com/acme/a/pull/1",
        "customfield_10043": ["https://github.com/acme/b/pull/2"],
    });
    let mapped = map(&issue(fields), &[], &config);
    assert_eq!(
        normalized(&mapped),
        vec!["github.com/acme/a/pull/1", "github.com/acme/b/pull/2"]
    );
}

#[test]
fn same_pull_request_named_twice_in_custom_fields_is_one_link() {
    let value = json!([
        "https://github.com/acme/svc/pull/3",
        "https://github.com/acme/svc/pull/3/files",
    ]);
    let mapped = map(&issue(json!({"customfield_10042": value})), &[], &config());
    assert_eq!(mapped.pull_requests.len(), 1);
    assert_eq!(mapped.pull_requests[0].sources, vec!["custom_field"]);
}

#[test]
fn dev_panel_title_is_kept_when_custom_field_has_none() {
    let dev = [pr_entry("https://github.com/acme/svc/pull/3", "Dev title")];
    let fields = json!({"customfield_10042": "https://github.com/acme/svc/pull/3"});
    let mapped = map(&issue(fields), &dev, &config());
    assert_eq!(mapped.pull_requests[0].title, "Dev title");
    assert_eq!(
        mapped.pull_requests[0].sources,
        vec!["custom_field", "dev_panel"]
    );
}

#[test]
fn dev_panel_entries_without_a_pull_request_url_are_skipped() {
    let dev = [
        json!({"url": "", "name": "x"}),
        json!({"url": "https://github.com/acme/svc/issues/3", "name": "y"}),
    ];
    assert!(
        map(&issue(json!({})), &dev, &config())
            .pull_requests
            .is_empty()
    );
}

#[test]
fn pull_requests_are_sorted_by_normalized_url() {
    let dev = [
        pr_entry("https://github.com/zeta/svc/pull/1", ""),
        pr_entry("https://github.com/alpha/svc/pull/2", ""),
    ];
    let mapped = map(&issue(json!({})), &dev, &config());
    assert_eq!(
        normalized(&mapped),
        vec!["github.com/alpha/svc/pull/2", "github.com/zeta/svc/pull/1"]
    );
}

// --- ADF flattening --------------------------------------------------------------------------------

#[test]
fn adf_flattening_covers_the_common_nodes() {
    let doc = json!({
        "type": "doc",
        "content": [
            {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Goal"}]},
            {
                "type": "orderedList",
                "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "one"}]}]},
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "two"}]}]},
                ]
            },
            {"type": "codeBlock", "content": [{"type": "text", "text": "x = 1"}]},
            {
                "type": "paragraph",
                "content": [
                    {"type": "text", "text": "docs", "marks": [{"type": "link", "attrs": {"href": "https://x.test/d"}}]},
                    {"type": "emoji", "attrs": {"shortName": ":tada:", "text": "🎉"}},
                ]
            },
            {"type": "paragraph", "content": []},
        ],
    });
    assert_eq!(adf_to_text(&doc), "Goal\n\n- one\n- two\n\nx = 1\n\ndocs🎉");
}

#[test]
fn adf_to_text_of_non_dict_is_empty() {
    assert_eq!(adf_to_text(&Value::Null), "");
    assert_eq!(adf_to_text(&json!("plain")), "");
}

// --- pull request URL parsing ----------------------------------------------------------------------

#[test]
fn valid_pull_request_urls() {
    for (url, normalized_url, repo_full_name, repo_normalized_url, number) in [
        (
            "https://github.com/org/repo/pull/42",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/files",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/commits",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.com/org/repo/pull/42/?tab=checks#discussion_r1",
            "github.com/org/repo/pull/42",
            "org/repo",
            "github.com/org/repo",
            42,
        ),
        (
            "http://www.GitHub.com/Org/Repo/pull/42",
            "github.com/org/repo/pull/42",
            "Org/Repo",
            "github.com/org/repo",
            42,
        ),
        (
            "https://github.example.com/Team/Svc/pull/7",
            "github.example.com/team/svc/pull/7",
            "Team/Svc",
            "github.example.com/team/svc",
            7,
        ),
    ] {
        let reference = parse_pull_request_url(url).expect(url);
        assert_eq!(reference.normalized_url, normalized_url, "{url}");
        assert_eq!(reference.repo_full_name, repo_full_name, "{url}");
        assert_eq!(reference.repo_normalized_url, repo_normalized_url, "{url}");
        assert_eq!(reference.number, number, "{url}");
        assert_eq!(reference.provider, "github");
        assert!(reference.sources.is_empty());
        assert_eq!(reference.title, "");
    }
}

#[test]
fn repo_normalized_url_matches_the_remote_normalizer() {
    let reference = parse_pull_request_url("https://github.com/Org/Repo/pull/1").expect("url");
    assert_eq!(
        reference.repo_normalized_url,
        wg_registry::remote::normalize_remote_url("https://github.com/org/repo.git")
    );
}

#[test]
fn invalid_pull_request_urls() {
    for url in [
        "https://github.com/org/repo/issues/42",
        "https://github.com/org/repo",
        "https://github.com/org/repo/",
        "https://github.com/org/repo/pull/",
        "https://github.com/org/repo/pull/abc",
        "https://github.com/org/repo/pull/0",
        "https://github.com/org/repo/pull/42/reviews",
        "https://gist.github.com/user/abc123",
        "https://github.com/org/repo/pulls",
        "github.com/org/repo/pull/42",
        "ftp://github.com/org/repo/pull/42",
        "not a url",
        "",
    ] {
        assert!(parse_pull_request_url(url).is_none(), "{url}");
    }
}
