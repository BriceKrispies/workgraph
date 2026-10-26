//! UI01: the pure rules of `ui open`, with no socket, process or browser.

#![allow(clippy::expect_used)]

use serde_json::{Map, Value as Json, json};
use wg_json::Value;
use wg_uispec::{Platform, Start, Wait};

fn open_matches(args: &[&str]) -> Result<wg_uispec::OpenRequest, String> {
    let mut argv = vec!["ui", "open"];
    argv.extend_from_slice(args);
    let matches = wg_uispec::command()
        .try_get_matches_from(argv)
        .expect("the arguments parse");
    let (_, open) = matches.subcommand().expect("ui has a subcommand");
    wg_uispec::open_request(open)
}

// covers: C192
#[test]
fn open_arguments_read_page_url_flags_and_timeout() {
    let request = open_matches(&["tasks/WG-12", "--url", "http://127.0.0.1:9/", "--print"])
        .expect("the request reads");
    assert_eq!(request.page, "tasks/WG-12");
    assert_eq!(request.url.as_deref(), Some("http://127.0.0.1:9/"));
    assert!(request.print && !request.no_start);
    assert!((request.timeout - 20.0).abs() < f64::EPSILON);

    let bare = open_matches(&["--no-start", "--timeout", "2.5"]).expect("the request reads");
    assert_eq!(bare.page, "");
    assert_eq!(bare.url, None);
    assert!(bare.no_start && !bare.print);
    assert!((bare.timeout - 2.5).abs() < f64::EPSILON);
}

// covers: C192
#[test]
fn a_timeout_that_is_not_a_number_is_a_usage_message() {
    let err = open_matches(&["--timeout", "soon"]).expect_err("rejected");
    assert!(err.contains("--timeout") && err.contains("soon"), "{err}");
    assert!(open_matches(&["--timeout", "nan"]).is_err());
}

// covers: C192
#[test]
fn ui_group_lists_open_and_screenshot() {
    let names: Vec<String> = wg_uispec::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    assert_eq!(names, ["open", "screenshot"]);
}

// covers: C192
#[test]
fn base_urls_end_with_one_slash() {
    assert_eq!(wg_uispec::normalize_base("http://h:1"), "http://h:1/");
    assert_eq!(wg_uispec::normalize_base("http://h:1///"), "http://h:1/");
}

// covers: C192
#[test]
fn base_comes_from_the_configured_server() {
    let config: Map<String, Json> = serde_json::from_value(json!({
        "server": {"host": "127.0.0.1", "port": 8800}
    }))
    .expect("a map");
    assert_eq!(
        wg_uispec::base_from_config(&config).expect("the base"),
        "http://127.0.0.1:8800/"
    );
    let missing: Map<String, Json> = Map::new();
    assert!(wg_uispec::base_from_config(&missing).is_err());
}

// covers: C192
#[test]
fn page_urls_join_under_the_base() {
    let base = "http://127.0.0.1:8800/";
    assert_eq!(wg_uispec::target_url(base, ""), "http://127.0.0.1:8800/");
    assert_eq!(
        wg_uispec::target_url(base, "/decisions"),
        "http://127.0.0.1:8800/decisions"
    );
    assert_eq!(
        wg_uispec::target_url(base, "graph?repo=rho&linked=1"),
        "http://127.0.0.1:8800/graph?repo=rho&linked=1"
    );
    assert_eq!(
        wg_uispec::health_url(base),
        "http://127.0.0.1:8800/api/v1/health"
    );
}

// covers: C192
#[test]
fn the_probe_is_a_plain_http_get_of_the_health_path() {
    let probe =
        wg_uispec::probe_of(&wg_uispec::health_url("http://127.0.0.1:8800/")).expect("probes");
    assert_eq!(probe.authority, "127.0.0.1:8800");
    assert_eq!(probe.path, "/api/v1/health");
    assert_eq!(
        wg_uispec::request_text(&probe),
        "GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:8800\r\nConnection: close\r\n\r\n"
    );
    assert!(wg_uispec::probe_of("https://example.test/api/v1/health").is_err());
    assert!(wg_uispec::probe_of("http:///api/v1/health").is_err());
}

// covers: C192
#[test]
fn only_a_success_status_counts_as_answering() {
    assert_eq!(wg_uispec::status_code("HTTP/1.1 200 OK"), Some(200));
    assert!(wg_uispec::answers_line("HTTP/1.1 204 No Content"));
    assert!(!wg_uispec::answers_line("HTTP/1.1 404 Not Found"));
    assert!(!wg_uispec::answers_line("HTTP/1.1 302 Found"));
    assert!(!wg_uispec::answers_line("SSH-2.0-OpenSSH"));
    assert!(!wg_uispec::answers_line(""));
}

// covers: C192
#[test]
fn a_running_server_is_used_and_never_started() {
    assert_eq!(
        wg_uispec::start_decision(true, false, false),
        Start::UseRunning
    );
    assert_eq!(
        wg_uispec::start_decision(true, true, true),
        Start::UseRunning
    );
}

// covers: C192
#[test]
fn nothing_answering_starts_a_server_unless_url_or_no_start_forbids_it() {
    assert_eq!(
        wg_uispec::start_decision(false, false, false),
        Start::Launch
    );
    assert_eq!(wg_uispec::start_decision(false, true, false), Start::Refuse);
    assert_eq!(wg_uispec::start_decision(false, false, true), Start::Refuse);
}

// covers: C192
#[test]
fn waiting_for_a_started_server_checks_answer_then_exit_then_deadline() {
    assert_eq!(wg_uispec::wait_step(true, Some(1), 99.0, 1.0), Wait::Ready);
    assert_eq!(
        wg_uispec::wait_step(false, Some(3), 0.0, 20.0),
        Wait::Exited(3)
    );
    assert_eq!(
        wg_uispec::wait_step(false, None, 20.0, 20.0),
        Wait::TimedOut
    );
    assert_eq!(wg_uispec::wait_step(false, None, 1.0, 20.0), Wait::Keep);
}

// covers: C192
#[test]
fn serve_is_started_with_the_home_it_was_given() {
    assert_eq!(wg_uispec::serve_args(None), ["serve"]);
    assert_eq!(
        wg_uispec::serve_args(Some("C:/w")),
        ["--home", "C:/w", "serve"]
    );
}

// covers: C192
#[test]
fn each_platform_opens_the_url_as_one_argument() {
    let url = "http://127.0.0.1:8800/graph?repo=a&b=c";
    assert_eq!(
        wg_uispec::browser_command(Platform::Windows, url),
        ("explorer", vec![url.to_owned()])
    );
    assert_eq!(
        wg_uispec::browser_command(Platform::MacOs, url),
        ("open", vec![url.to_owned()])
    );
    assert_eq!(
        wg_uispec::browser_command(Platform::Other, url),
        ("xdg-open", vec![url.to_owned()])
    );
}

// covers: C192
#[test]
fn messages_and_results_match_the_python_wording() {
    assert_eq!(
        wg_uispec::no_server_message("http://127.0.0.1:8800/"),
        "no Workgraph server at http://127.0.0.1:8800/; start one with `workgraph serve`"
    );
    assert_eq!(
        wg_uispec::exited_message(1, "/logs/serve-detached.log"),
        "`workgraph serve` exited with code 1; see /logs/serve-detached.log"
    );
    assert_eq!(
        wg_uispec::timed_out_message("http://h:1/", 20.0, "/l"),
        "`workgraph serve` did not answer at http://h:1/ within 20s; see /l"
    );
    assert_eq!(
        wg_uispec::human("http://h:1/x", "http://h:1/", true, false),
        "started server at http://h:1/\nopened http://h:1/x\n"
    );
    assert_eq!(
        wg_uispec::human("http://h:1/x", "http://h:1/", false, true),
        "http://h:1/x\n"
    );
    let Value::Dict(fields) = wg_uispec::opened_value("http://h:1/", false) else {
        panic!("a dict");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[1].1, Value::Bool(false));
}
