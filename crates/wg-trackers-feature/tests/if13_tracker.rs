//! The `tracker` command group (IF-13): the clap tree, the connection and status documents, and
//! the human views, checked without a database.

use serde_json::json;
use wg_render::Console;
use wg_trackers_feature::spec::{self, Connection, Status, args, view};

/// Two connections, configured out of name order.
fn config() -> serde_json::Map<String, serde_json::Value> {
    let value = json!({
        "trackers": {
            "ops": {
                "type": "jira",
                "flavour": "data_center",
                "base_url": "https://ops.example.test",
                "workspace": "ops",
                "projects": ["OPS", "INF"],
                "token_env": "OPS_TOKEN"
            },
            "acme": {
                "type": "jira",
                "flavour": "cloud",
                "base_url": "https://acme.example.test",
                "workspace": "acme",
                "projects": ["ACME"],
                "token_env": "ACME_TOKEN"
            }
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

fn synced_status() -> Status {
    Status {
        connection: "acme".to_owned(),
        workspace: "acme".to_owned(),
        projects: vec!["ACME".to_owned()],
        cursor: Some(1_700_000_000_123),
        last_sync: Some(1_700_000_000_000),
        last_full_sync: None,
        tracked_tasks: 3,
        missing: 0,
        unresolved_pull_request_repos: Vec::new(),
    }
}

// covers: C168
#[test]
fn tracker_tree_has_list_status_and_sync() {
    let command = args::command();
    let names: Vec<&str> = command
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect();
    assert_eq!(names, ["list", "status", "sync"]);
}

// covers: C170
#[test]
fn sync_takes_a_connection_and_the_two_flags() {
    let command = args::command();
    let sync = command
        .get_subcommands()
        .find(|sub| sub.get_name() == "sync");
    let Some(sync) = sync else {
        panic!("tracker sync is missing from the tree");
    };
    let flags: Vec<&str> = sync
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .collect();
    assert!(flags.contains(&"full"), "--full: {flags:?}");
    assert!(flags.contains(&"queue"), "--queue: {flags:?}");
    let connection = sync
        .get_arguments()
        .find(|arg| arg.get_id() == "connection");
    assert!(connection.is_some_and(|arg| arg.is_required_set()));
}

// covers: C168
#[test]
fn connections_are_listed_in_name_order_with_their_projects() {
    let rows = spec::connections(&config());
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["acme", "ops"]);
    assert_eq!(rows[1].projects, ["OPS", "INF"]);
    assert_eq!(rows[1].flavour, "data_center");
}

// covers: C169
#[test]
fn status_document_follows_the_python_key_order() {
    let document = spec::status_out(&synced_status());
    let encoded = wg_json::cli_json(&document).unwrap_or_default();
    let keys = [
        "connection",
        "workspace",
        "projects",
        "cursor",
        "last_sync",
        "last_full_sync",
        "tracked_tasks",
        "missing",
        "unresolved_pull_request_repos",
    ];
    let mut at = 0;
    for key in keys {
        let found = encoded[at..].find(&format!("\"{key}\""));
        let Some(offset) = found else {
            panic!("{key} is missing or out of order in {encoded}");
        };
        at += offset + key.len();
    }
    assert!(encoded.contains("\"cursor\": \"2023-11-14T22:13:20.123000+00:00\""));
    assert!(encoded.contains("\"last_full_sync\": null"));
}

// covers: C169
#[test]
fn a_named_status_is_one_document_and_all_is_a_list() {
    let statuses = vec![synced_status()];
    assert!(matches!(
        spec::status_document(&statuses, true),
        wg_json::Value::Dict(_)
    ));
    assert!(matches!(
        spec::status_document(&statuses, false),
        wg_json::Value::List(_)
    ));
}

// covers: C169
#[test]
fn whole_second_cursors_have_no_fraction() {
    assert_eq!(
        spec::iso_of_millis(1_700_000_000_000).as_deref(),
        Some("2023-11-14T22:13:20+00:00")
    );
}

// covers: C168
#[test]
fn an_empty_list_says_nothing_to_show() {
    let console = Console::new(80);
    let text = view::list_view(&console, &[]);
    assert!(text.contains("nothing to show"), "{text}");
}

// covers: C168
#[test]
fn list_view_joins_projects_with_commas() {
    let rows = vec![Connection {
        name: "ops".to_owned(),
        flavour: "cloud".to_owned(),
        base_url: "https://ops.example.test".to_owned(),
        workspace: "ops".to_owned(),
        projects: vec!["OPS".to_owned(), "INF".to_owned()],
    }];
    let text = view::list_view(&Console::new(120), &rows);
    assert!(text.contains("OPS,INF"), "{text}");
}

// covers: C169
#[test]
fn status_view_leaves_out_empty_values() {
    let text = view::status_view(&Console::new(80), &[synced_status()]);
    assert!(text.starts_with("tracker acme"), "{text}");
    assert!(text.contains("  projects: ['ACME']"), "{text}");
    assert!(text.contains("  tracked_tasks: 3"), "{text}");
    assert!(!text.contains("unresolved_pull_request_repos"), "{text}");
    assert!(!text.contains("last_full_sync"), "{text}");
}

// covers: C170
#[test]
fn enqueued_reply_carries_the_job_connection_and_mode() {
    let document = spec::enqueued_out("job-1", "acme", true);
    let encoded = wg_json::cli_json(&document).unwrap_or_default();
    assert!(encoded.contains("\"job_id\": \"job-1\""), "{encoded}");
    assert!(encoded.contains("\"connection\": \"acme\""), "{encoded}");
    assert!(encoded.contains("\"full\": true"), "{encoded}");
}

// covers: C168
#[test]
fn list_document_has_one_row_per_connection_with_python_keys() {
    let rows = spec::connections(&config());
    let encoded = wg_json::cli_json(&spec::list_out(&rows)).unwrap_or_default();
    let keys = [
        "\"name\": \"acme\"",
        "\"flavour\": \"cloud\"",
        "\"base_url\": \"https://acme.example.test\"",
        "\"workspace\": \"acme\"",
        "\"projects\": [",
        "\"name\": \"ops\"",
    ];
    let mut at = 0;
    for key in keys {
        let Some(offset) = encoded[at..].find(key) else {
            panic!("{key} is missing or out of order in {encoded}");
        };
        at += offset + key.len();
    }
}

// covers: C169
#[test]
fn an_unknown_connection_is_not_found_with_the_python_message() {
    let rows = spec::connections(&config());
    let Err(err) = spec::connection_of(&rows, "nope") else {
        panic!("an unconfigured connection must be refused");
    };
    assert_eq!(err.code, wg_types::ErrorCode::NotFound);
    assert_eq!(err.message, "tracker connection 'nope' is not configured");
    assert_eq!(
        err.details.get("connection").and_then(|v| v.as_str()),
        Some("nope")
    );
}

// covers: C170
#[test]
fn sync_document_has_the_report_keys_in_python_order() {
    let mut report = wg_trackers::SyncReport::new("acme", true);
    report.fetched = 4;
    report.created = 2;
    report.cursor = Some(1_700_000_000_123);
    let encoded = wg_json::cli_json(&spec::run::sync_out(&report)).unwrap_or_default();
    let keys = [
        "connection",
        "full",
        "fetched",
        "created",
        "updated",
        "unchanged",
        "missing",
        "archived",
        "unresolved_pull_requests",
        "errors",
        "cursor",
    ];
    let mut at = 0;
    for key in keys {
        let Some(offset) = encoded[at..].find(&format!("\"{key}\"")) else {
            panic!("{key} is missing or out of order in {encoded}");
        };
        at += offset + key.len();
    }
    assert!(encoded.contains("\"full\": true"), "{encoded}");
    assert!(
        encoded.contains("\"cursor\": \"2023-11-14T22:13:20.123000+00:00\""),
        "{encoded}"
    );
}

// covers: C170
#[test]
fn a_report_without_a_cursor_writes_null() {
    let report = wg_trackers::SyncReport::new("acme", false);
    let encoded = wg_json::cli_json(&spec::run::sync_out(&report)).unwrap_or_default();
    assert!(encoded.contains("\"cursor\": null"), "{encoded}");
    assert!(
        encoded.contains("\"unresolved_pull_requests\": []"),
        "{encoded}"
    );
}

// covers: C170
#[test]
fn sync_view_prints_the_python_labels_and_leaves_out_empty_lists() {
    let mut report = wg_trackers::SyncReport::new("acme", false);
    report.fetched = 1;
    let text = view::sync_view(&Console::new(80), &report);
    assert!(text.starts_with("tracker sync"), "{text}");
    assert!(text.contains("  full: False"), "{text}");
    assert!(text.contains("  fetched: 1"), "{text}");
    assert!(text.contains("  connection: acme"), "{text}");
    assert!(!text.contains("unresolved pull requests"), "{text}");
    assert!(!text.contains("errors"), "{text}");
    assert!(!text.contains("cursor"), "{text}");
}

// covers: C170
#[test]
fn sync_view_prints_the_error_list_as_a_python_repr() {
    let mut report = wg_trackers::SyncReport::new("acme", true);
    report.errors = vec!["boom".to_owned()];
    let text = view::sync_view(&Console::new(80), &report);
    assert!(text.contains("  full: True"), "{text}");
    assert!(text.contains("  errors: ['boom']"), "{text}");
}

// covers: C170
#[test]
fn enqueued_line_names_the_job() {
    let text = view::enqueued_line(&Console::new(80), "job-7");
    assert!(text.contains("enqueued job-7"), "{text}");
}

// covers: C169
#[test]
fn status_view_prints_one_block_per_status_in_order() {
    let mut second = synced_status();
    second.connection = "ops".to_owned();
    let text = view::status_view(&Console::new(80), &[synced_status(), second]);
    let first_at = text.find("tracker acme");
    assert!(first_at.is_some(), "{text}");
    assert!(text.find("tracker ops") > first_at, "{text}");
}
