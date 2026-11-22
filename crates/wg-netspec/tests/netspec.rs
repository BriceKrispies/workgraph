#![allow(clippy::expect_used, clippy::unwrap_used)]
//! The pure parts of the HTTP clients: JQL and its timestamps, the retry decisions and the
//! reading of responses.

use rpds::VectorSync;
use serde_json::json;
use wg_model::{EffectError, EffectErrorKind, HttpResponse};
use wg_netspec::jira::{Cursor, Flavour};
use wg_netspec::jql::{jql_minute, jql_string, scoped_jql, updated_jql};
use wg_netspec::request::url;
use wg_netspec::retry::{
    Attempt, Backoff, JiraStep, Step, jira_delay, jira_request_gap, jira_step, provider_step,
};
use wg_netspec::text::percent_encode;
use wg_netspec::{ollama, openai};

fn response(status: u16, retry_after: Option<&str>) -> Result<HttpResponse, EffectError> {
    let headers: VectorSync<(String, String)> = retry_after
        .map(|value| ("retry-after".to_owned(), value.to_owned()))
        .into_iter()
        .collect();
    Ok(HttpResponse {
        status,
        headers,
        body: "busy".to_owned(),
    })
}

#[test]
fn jql_strings_escape_backslashes_and_quotes() {
    assert_eq!(jql_string(r#"a"b\c"#), r#""a\"b\\c""#);
}

#[test]
fn jql_minute_applies_the_offset_and_drops_seconds() {
    // 2026-10-18 09:30:45 UTC at UTC-5 (Chicago, CDT) is 04:30.
    assert_eq!(jql_minute(1_792_315_845, -5 * 3_600), "2026-10-18 04:30");
    // The same instant at UTC is 09:30.
    assert_eq!(jql_minute(1_792_315_845, 0), "2026-10-18 09:30");
    // Before the epoch, and across a day boundary.
    assert_eq!(jql_minute(-1, 0), "1969-12-31 23:59");
    assert_eq!(
        jql_minute(1_792_281_600 + 23 * 3_600 + 59 * 60, 3_600),
        "2026-10-19 00:59"
    );
}

#[test]
fn updated_jql_adds_the_clause_only_with_a_stamp() {
    let projects = vec!["ABC".to_owned(), "DEF".to_owned()];
    assert_eq!(
        updated_jql(&projects, None, None),
        r#"project in ("ABC","DEF") ORDER BY updated ASC"#
    );
    assert_eq!(
        updated_jql(&projects, None, Some("2026-10-18 04:30")),
        r#"project in ("ABC","DEF") AND updated >= "2026-10-18 04:30" ORDER BY updated ASC"#
    );
}

#[test]
fn a_scope_is_anded_on_in_parentheses_and_a_blank_one_is_ignored() {
    let projects = vec!["ABC".to_owned()];
    assert_eq!(
        updated_jql(
            &projects,
            Some("labels = x OR labels = y"),
            Some("2026-10-18 04:30")
        ),
        r#"project in ("ABC") AND (labels = x OR labels = y) AND updated >= "2026-10-18 04:30" ORDER BY updated ASC"#
    );
    assert_eq!(scoped_jql(&projects, Some("  ")), r#"project in ("ABC")"#);
}

#[test]
fn urls_drop_the_base_slash_and_encode_the_query() {
    let query = vec![("jql".to_owned(), "a b&c".to_owned())];
    assert_eq!(
        url("http://h:1/", "/x", &query),
        "http://h:1/x?jql=a%20b%26c"
    );
    assert_eq!(percent_encode("A-z_0.~"), "A-z_0.~");
}

#[test]
fn provider_retries_stop_after_max_retries_and_double_the_wait() {
    let backoff = Backoff {
        max_retries: 2,
        base_seconds: 1.0,
    };
    let busy = Attempt::of(&response(503, None));
    assert_eq!(
        provider_step(&busy, 1, backoff),
        Step::Retry { delay_seconds: 1.0 }
    );
    assert_eq!(
        provider_step(&busy, 2, backoff),
        Step::Retry { delay_seconds: 2.0 }
    );
    assert_eq!(provider_step(&busy, 3, backoff), Step::Finished);
    // A status outside the retryable set is final at once.
    let missing = Attempt::of(&response(404, None));
    assert_eq!(provider_step(&missing, 1, backoff), Step::Finished);
}

#[test]
fn provider_transport_failures_are_retried_but_unsendable_calls_are_not() {
    let backoff = Backoff {
        max_retries: 1,
        base_seconds: 0.5,
    };
    let refused = Attempt::of(&Err(EffectError {
        kind: EffectErrorKind::Unavailable,
        message: "refused".to_owned(),
    }));
    assert_eq!(
        provider_step(&refused, 1, backoff),
        Step::Retry { delay_seconds: 0.5 }
    );
    let malformed = Attempt::of(&Err(EffectError {
        kind: EffectErrorKind::Failed,
        message: "bad uri".to_owned(),
    }));
    assert_eq!(provider_step(&malformed, 1, backoff), Step::Finished);
}

#[test]
fn jira_retries_429_and_5xx_except_501_and_honours_retry_after() {
    let limited = Attempt::of(&response(429, Some("7")));
    assert_eq!(
        jira_step(&limited, 1, false),
        JiraStep::Retry { delay_seconds: 7.0 }
    );
    assert_eq!(
        jira_step(&Attempt::of(&response(501, None)), 1, false),
        JiraStep::Finished
    );
    assert_eq!(
        jira_step(&Attempt::of(&response(500, None)), 4, false),
        JiraStep::Finished
    );
    assert_eq!(
        jira_step(&Attempt::of(&response(500, None)), 3, false),
        JiraStep::Retry { delay_seconds: 2.0 }
    );
    let too_long = Attempt::of(&response(429, Some("301")));
    assert_eq!(jira_step(&too_long, 1, false), JiraStep::RetryAfterTooLong);
}

#[test]
fn jira_reauthenticates_a_401_once() {
    let rejected = Attempt::of(&response(401, None));
    assert_eq!(jira_step(&rejected, 1, false), JiraStep::Reauthenticate);
    assert_eq!(jira_step(&rejected, 1, true), JiraStep::Finished);
}

#[test]
fn jira_delay_honours_retry_after_up_to_five_minutes() {
    assert_eq!(jira_delay(Some("120"), 1), Some(120.0));
    assert_eq!(jira_delay(Some("300"), 1), Some(300.0));
    assert_eq!(jira_delay(Some("301"), 1), None);
    assert_eq!(jira_delay(Some("-4"), 1), Some(0.0));
    assert_eq!(jira_delay(Some("soon"), 2), Some(1.0));
    assert_eq!(jira_delay(None, 1), Some(0.5));
    assert_eq!(jira_delay(None, 10), Some(30.0));
}

#[test]
fn near_limit_raises_the_gap_to_the_pause() {
    assert_eq!(jira_request_gap(0.5, None), 0.5);
    assert_eq!(jira_request_gap(0.5, Some("false")), 0.5);
    assert_eq!(jira_request_gap(0.5, Some("TRUE")), 5.0);
    assert_eq!(jira_request_gap(9.0, Some("true")), 9.0);
}

#[test]
fn cloud_auth_is_basic_with_an_email_and_bearer_without() {
    let header = |flavour: &Flavour| flavour.auth_headers()[0].1.clone();
    let basic = Flavour::Cloud {
        email: Some("e".to_owned()),
        token: "t".to_owned(),
    };
    assert!(header(&basic).starts_with("Basic "));
    let bearer = Flavour::Cloud {
        email: None,
        token: "t".to_owned(),
    };
    assert_eq!(header(&bearer), "Bearer t");
    assert_eq!(header(&bearer.with_token("u".to_owned())), "Bearer u");
    assert_eq!(bearer.key_list_fields(), "key");
}

#[test]
fn cloud_pages_end_on_is_last_or_a_missing_token() {
    let flavour = Flavour::Cloud {
        email: Some("e".to_owned()),
        token: "t".to_owned(),
    };
    let more = json!({"nextPageToken": "tok", "isLast": false});
    assert_eq!(
        flavour.next_cursor(&Cursor::Token(None), &more, 1),
        Some(Cursor::Token(Some("tok".to_owned())))
    );
    let last = json!({"nextPageToken": "tok", "isLast": true});
    assert_eq!(flavour.next_cursor(&Cursor::Token(None), &last, 1), None);
}

#[test]
fn data_center_pages_advance_until_the_total() {
    let flavour = Flavour::DataCenter {
        token: "t".to_owned(),
    };
    let page = json!({"total": 3});
    assert_eq!(
        flavour.next_cursor(&Cursor::Offset(0), &page, 2),
        Some(Cursor::Offset(2))
    );
    assert_eq!(flavour.next_cursor(&Cursor::Offset(2), &page, 1), None);
    assert_eq!(flavour.next_cursor(&Cursor::Offset(0), &page, 0), None);
}

#[test]
fn ollama_model_check_matches_with_or_without_the_latest_tag() {
    let models = vec!["alpha".to_owned(), "qwen:latest".to_owned()];
    assert_eq!(
        ollama::check_model(&models, "qwen"),
        (true, "qwen available".to_owned())
    );
    assert_eq!(
        ollama::check_model(&models, "alpha"),
        (true, "alpha available".to_owned())
    );
    let (found, message) = ollama::check_model(&[], "x");
    assert!(!found);
    assert!(
        message.ends_with("(available: none); run `ollama pull x`"),
        "{message}"
    );
}

#[test]
fn openai_model_ids_are_sorted_and_missing_ids_read_as_none() {
    let body = json!({"data": [{"id": "b"}, {}, {"id": "a"}]});
    assert_eq!(openai::model_ids(&body), ["None", "a", "b"]);
}

#[test]
fn ollama_chat_content_must_be_a_json_object() {
    let body = json!({"message": {"content": "[1, 2]"}});
    let error = ollama::parse_chat(&body, "m").expect_err("an array is not an object");
    assert_eq!(error.message(), "model output is not a JSON object");
    let body = json!({"message": {"content": "{\"a\": 1}"}, "model": "served"});
    let parsed = ollama::parse_chat(&body, "m").expect("object parses");
    assert_eq!(parsed.model, "served");
    assert_eq!(parsed.data["a"], 1);
}
