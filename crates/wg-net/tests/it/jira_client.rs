#![allow(clippy::expect_used, clippy::unwrap_used)]
//! The Jira client against an in-process server: ports `tests/unit/test_jira_client.py`.

use crate::common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use wg_net::{JiraClient, Net};
use wg_netspec::jira::Flavour;

use common::{Fake, Reply, delays_of, net, recording_sleep};

const EMAIL: &str = "bot@example.com";
const TOKEN: &str = "s3cret-token";

/// 2026-10-18 00:00 UTC, in epoch seconds.
const OCT_18: i64 = 1_792_281_600;

fn cloud() -> Flavour {
    Flavour::Cloud {
        email: EMAIL.to_owned(),
        token: TOKEN.to_owned(),
    }
}

fn data_center() -> Flavour {
    Flavour::DataCenter {
        token: TOKEN.to_owned(),
    }
}

/// A Jira client for `url` that sends through `net`, with a 5-second timeout.
fn jira(url: &str, flavour: Flavour, net: Net) -> JiraClient {
    JiraClient {
        base_url: url.to_owned(),
        flavour,
        timeout_seconds: 5.0,
        net,
        zone: None,
    }
}

fn projects(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn issue(key: &str, number: u64) -> Value {
    json!({"id": number.to_string(), "key": key, "fields": {"summary": key}})
}

fn page(issues: Vec<Value>) -> Reply {
    Reply::json(200, &json!({"issues": issues, "isLast": true}))
}

/// Answers `/myself` with `timeZone` (omitted when None) and every search with no issues.
fn myself_handler(
    time_zone: Option<&str>,
) -> impl Fn(&common::Recorded) -> Reply + Send + Sync + use<> {
    let zone = time_zone.map(str::to_owned);
    move |request| {
        if request.path().ends_with("/myself") {
            let mut body = json!({"accountId": "abc"});
            if let Some(zone) = &zone {
                body["timeZone"] = json!(zone);
            }
            Reply::json(200, &body)
        } else {
            page(Vec::new())
        }
    }
}

#[test]
fn cloud_search_follows_next_page_token() {
    let fake = Fake::start(|request| {
        if request.query("nextPageToken").is_none() {
            Reply::json(
                200,
                &json!({"issues": [issue("ABC-1", 1)], "nextPageToken": "tok2", "isLast": false}),
            )
        } else {
            page(vec![issue("ABC-2", 2)])
        }
    });
    let mut client = jira(&fake.url, cloud(), Net::default());

    let issues = client
        .search_updated(&projects(&["ABC"]), None)
        .expect("search succeeds");

    let keys: Vec<&str> = issues.iter().filter_map(|i| i["key"].as_str()).collect();
    assert_eq!(keys, ["ABC-1", "ABC-2"]);
    let seen = fake.requests();
    assert!(seen.iter().all(|r| r.path() == "/rest/api/3/search/jql"));
    assert_eq!(seen[0].query("fields").as_deref(), Some("*all"));
    assert_eq!(seen[0].query("maxResults").as_deref(), Some("100"));
    assert_eq!(seen[0].query("nextPageToken"), None);
    assert_eq!(seen[1].query("nextPageToken").as_deref(), Some("tok2"));
}

#[test]
fn cloud_list_keys_requests_id_fields() {
    let fake = Fake::start(|_| page(vec![issue("ABC-1", 10001)]));
    let mut client = jira(&fake.url, cloud(), Net::default());

    let keys = client
        .list_keys(&projects(&["ABC"]))
        .expect("list succeeds");

    assert_eq!(keys, [("10001".to_owned(), "ABC-1".to_owned())]);
    assert_eq!(fake.requests()[0].query("fields").as_deref(), Some("id"));
}

#[test]
fn data_center_search_pages_by_start_at() {
    let fake = Fake::start(|request| {
        let (keys, total): (&[&str], u64) = match request.query("startAt").as_deref() {
            Some("0") => (&["ABC-1", "ABC-2"], 3),
            Some("2") => (&["ABC-3"], 3),
            _ => (&[], 3),
        };
        let issues: Vec<Value> = keys
            .iter()
            .enumerate()
            .map(|(n, key)| issue(key, n as u64))
            .collect();
        Reply::json(
            200,
            &json!({"startAt": 0, "total": total, "issues": issues}),
        )
    });
    let mut client = jira(&fake.url, data_center(), Net::default());

    let issues = client
        .search_updated(&projects(&["ABC"]), None)
        .expect("search succeeds");

    let keys: Vec<&str> = issues.iter().filter_map(|i| i["key"].as_str()).collect();
    assert_eq!(keys, ["ABC-1", "ABC-2", "ABC-3"]);
    let seen = fake.requests();
    assert!(seen.iter().all(|r| r.path() == "/rest/api/2/search"));
    let starts: Vec<String> = seen.iter().filter_map(|r| r.query("startAt")).collect();
    assert_eq!(starts, ["0", "2"]);
}

#[test]
fn jql_without_since_has_no_updated_clause() {
    let fake = Fake::start(|_| page(Vec::new()));
    let mut client = jira(&fake.url, cloud(), Net::default());

    let issues = client
        .search_updated(&projects(&["ABC", "DEF"]), None)
        .expect("search succeeds");

    assert!(issues.is_empty());
    assert_eq!(
        fake.requests()[0].query("jql").as_deref(),
        Some("project in (\"ABC\",\"DEF\") ORDER BY updated ASC")
    );
}

#[test]
fn jql_with_since_uses_the_user_timezone_and_caches_it() {
    let fake = Fake::start(myself_handler(Some("America/Chicago")));
    let mut client = jira(&fake.url, cloud(), Net::default());
    // 2026-10-18 09:30:45 UTC is 04:30 in Chicago (CDT, UTC-5).
    let first = OCT_18 + 9 * 3_600 + 30 * 60 + 45;
    client
        .search_updated(&projects(&["ABC", "DEF"]), Some(first))
        .expect("first search");
    // 09:30 UTC again, the same minute in Chicago.
    let second = OCT_18 + 9 * 3_600 + 30 * 60;
    client
        .search_updated(&projects(&["ABC"]), Some(second))
        .expect("second search");

    let seen = fake.requests();
    assert_eq!(
        seen.iter()
            .filter(|r| r.path() == "/rest/api/3/myself")
            .count(),
        1
    );
    let searches: Vec<String> = seen
        .iter()
        .filter(|r| r.path() == "/rest/api/3/search/jql")
        .filter_map(|r| r.query("jql"))
        .collect();
    assert_eq!(
        searches,
        [
            "project in (\"ABC\",\"DEF\") AND updated >= \"2026-10-18 04:30\" ORDER BY updated ASC",
            "project in (\"ABC\") AND updated >= \"2026-10-18 04:30\" ORDER BY updated ASC",
        ]
    );
}

#[test]
fn missing_timezone_falls_back_to_utc() {
    let fake = Fake::start(myself_handler(None));
    let mut client = jira(&fake.url, cloud(), Net::default());
    let since = OCT_18 + 9 * 3_600 + 30 * 60;

    client
        .search_updated(&projects(&["ABC"]), Some(since))
        .expect("search succeeds");

    let jql = fake.requests().last().and_then(|r| r.query("jql"));
    assert_eq!(
        jql.as_deref(),
        Some("project in (\"ABC\") AND updated >= \"2026-10-18 09:30\" ORDER BY updated ASC")
    );
}

#[test]
fn unknown_timezone_falls_back_to_utc() {
    let fake = Fake::start(myself_handler(Some("Mars/Olympus_Mons")));
    let mut client = jira(&fake.url, cloud(), Net::default());
    let since = OCT_18 + 9 * 3_600 + 30 * 60;

    client
        .search_updated(&projects(&["ABC"]), Some(since))
        .expect("search succeeds");

    let jql = fake.requests().last().and_then(|r| r.query("jql"));
    assert_eq!(
        jql.as_deref(),
        Some("project in (\"ABC\") AND updated >= \"2026-10-18 09:30\" ORDER BY updated ASC")
    );
}

#[test]
fn data_center_reads_its_own_myself_endpoint() {
    let fake = Fake::start(myself_handler(Some("Europe/Paris")));
    let mut client = jira(&fake.url, data_center(), Net::default());
    let since = OCT_18 + 9 * 3_600 + 30 * 60;

    client
        .search_updated(&projects(&["ABC"]), Some(since))
        .expect("search succeeds");

    let seen = fake.requests();
    assert_eq!(seen[0].path(), "/rest/api/2/myself");
    // Paris is CEST (UTC+2) in October.
    assert_eq!(
        seen[1].query("jql").as_deref(),
        Some("project in (\"ABC\") AND updated >= \"2026-10-18 11:30\" ORDER BY updated ASC")
    );
}

#[test]
fn cloud_uses_basic_auth_and_data_center_uses_bearer() {
    let fake = Fake::start(|_| Reply::json(200, &json!({"id": "1", "key": "ABC-1", "fields": {}})));
    let mut cloud_client = jira(&fake.url, cloud(), Net::default());
    cloud_client.get_issue("ABC-1").expect("request succeeds");
    let expected = format!("Basic {}", STANDARD.encode(format!("{EMAIL}:{TOKEN}")));
    let first = fake.requests().last().cloned().expect("one request");
    assert_eq!(first.header("Authorization"), Some(expected.as_str()));
    assert_eq!(first.header("Accept"), Some("application/json"));

    let mut dc_client = jira(&fake.url, data_center(), Net::default());
    dc_client.get_issue("ABC-1").expect("request succeeds");
    let last = fake.requests().last().cloned().expect("second request");
    assert_eq!(
        last.header("Authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(last.path(), "/rest/api/2/issue/ABC-1");
}

#[test]
fn retry_after_429_is_retried_after_its_seconds() {
    let replies = std::sync::Mutex::new(std::collections::VecDeque::from(vec![
        Reply::text(429, "").header("Retry-After", "7"),
        Reply::json(200, &json!({"id": "1", "key": "ABC-1", "fields": {}})),
    ]));
    let fake = Fake::start(move |_| {
        replies
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_else(|| Reply::text(500, "exhausted"))
    });
    let (sleep, delays) = recording_sleep();
    let mut client = jira(&fake.url, cloud(), net(sleep));

    let issue = client.get_issue("ABC-1").expect("retried to success");

    assert_eq!(
        issue
            .and_then(|i| i["key"].as_str().map(str::to_owned))
            .as_deref(),
        Some("ABC-1")
    );
    assert_eq!(delays_of(&delays), [7.0]);
    assert_eq!(fake.requests().len(), 2);
}

#[test]
fn status_501_is_not_retried() {
    let fake = Fake::start(|_| Reply::text(501, ""));
    let (sleep, delays) = recording_sleep();
    let mut client = jira(&fake.url, cloud(), net(sleep));

    let error = client.get_issue("ABC-1").expect_err("501 is an error");

    assert_eq!(error.status, Some(501));
    assert_eq!(fake.requests().len(), 1);
    assert!(delays_of(&delays).is_empty());
}

#[test]
fn persistent_500_raises_tracker_error_without_credentials() {
    let fake = Fake::start(|_| Reply::text(500, "boom"));
    let (sleep, delays) = recording_sleep();
    let mut client = jira(&fake.url, cloud(), net(sleep));

    let error = client.get_issue("ABC-1").expect_err("500 is an error");

    assert!(error.message.contains("500") && error.message.contains("/rest/api/3/issue/ABC-1"));
    let basic = STANDARD.encode(format!("{EMAIL}:{TOKEN}"));
    for secret in [TOKEN, EMAIL, "Basic ", basic.as_str()] {
        assert!(!error.message.contains(secret), "message leaks {secret:?}");
    }
    assert_eq!(fake.requests().len(), 4);
    assert_eq!(delays_of(&delays), [0.5, 1.0, 2.0]);
}

#[test]
fn get_issue_404_returns_none() {
    let fake = Fake::start(|_| Reply::json(404, &json!({"errorMessages": ["gone"]})));
    let mut client = jira(&fake.url, cloud(), Net::default());

    assert_eq!(
        client.get_issue("ABC-404").expect("404 is not an error"),
        None
    );
}

#[test]
fn dev_pull_requests_flattens_details() {
    let body = json!({
        "detail": [
            {"pullRequests": [{"url": "https://github.com/acme/a/pull/1"}]},
            {"pullRequests": [{"url": "https://github.com/acme/b/pull/2"}, {"url": "c"}]},
            {},
        ]
    });
    let fake = Fake::start(move |_| Reply::json(200, &body));
    let mut client = jira(&fake.url, cloud(), Net::default());

    let pulls = client.dev_pull_requests("10001");

    let urls: Vec<&str> = pulls.iter().filter_map(|p| p["url"].as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://github.com/acme/a/pull/1",
            "https://github.com/acme/b/pull/2",
            "c",
        ]
    );
    let request = &fake.requests()[0];
    assert_eq!(request.path(), "/rest/dev-status/latest/issue/detail");
    assert_eq!(request.query("issueId").as_deref(), Some("10001"));
    assert_eq!(request.query("applicationType").as_deref(), Some("GitHub"));
    assert_eq!(request.query("dataType").as_deref(), Some("pullrequest"));
}

#[test]
fn dev_pull_requests_error_returns_empty() {
    let fake = Fake::start(|_| Reply::text(403, ""));
    let mut client = jira(&fake.url, cloud(), Net::default());

    assert!(client.dev_pull_requests("10001").is_empty());
}

#[test]
fn every_request_is_a_get() {
    let fake = Fake::start(|request| {
        if request.path().starts_with("/rest/dev-status") {
            Reply::json(200, &json!({"detail": []}))
        } else if request.path().ends_with("/myself") {
            Reply::json(200, &json!({"timeZone": "UTC"}))
        } else if request.path().contains("/issue/") {
            Reply::json(200, &json!({"id": "1", "key": "ABC-1", "fields": {}}))
        } else {
            page(vec![issue("ABC-1", 1)])
        }
    });
    let mut client = jira(&fake.url, cloud(), Net::default());
    let since = OCT_18;

    client
        .search_updated(&projects(&["ABC"]), Some(since))
        .expect("search");
    client.list_keys(&projects(&["ABC"])).expect("keys");
    client.get_issue("ABC-1").expect("issue");
    client.dev_pull_requests("1");

    let seen = fake.requests();
    assert_eq!(seen.len(), 5);
    assert!(seen.iter().all(|r| r.method == "GET"));
}
