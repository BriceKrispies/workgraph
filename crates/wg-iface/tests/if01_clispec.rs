//! IF-01: the pure rules of the CLI skeleton: argv, exit codes (O05) and error reports (O02).

#![allow(clippy::expect_used)]

use serde_json::{Map, json};
use wg_iface::cli::{CliFailure, Kind, Snapshot, report, split_json};
use wg_render::Console;
use wg_types::{ErrorCode, WgError};

fn console() -> Console {
    Console::new(100)
}

#[test]
fn json_is_removed_everywhere() {
    let argv = split_json(["task", "--json", "list", "--json", "--jsonx"]);
    assert!(argv.json);
    assert_eq!(argv.args, ["task", "list", "--jsonx"]);
    let argv = split_json(["task", "list"]);
    assert!(!argv.json);
    assert_eq!(argv.args, ["task", "list"]);
}

#[test]
fn exit_codes_follow_the_error_class() {
    let cases = [
        (ErrorCode::ValidationFailed, 2),
        (ErrorCode::DependencyCycle, 2),
        (ErrorCode::Conflict, 3),
        (ErrorCode::InvalidTransition, 3),
        (ErrorCode::NotFound, 4),
        (ErrorCode::PermissionDenied, 5),
        (ErrorCode::ProviderUnavailable, 6),
        (ErrorCode::ConfigError, 7),
        (ErrorCode::GitError, 1),
        (ErrorCode::WorkgraphError, 1),
    ];
    for (code, exit) in cases {
        let failure = CliFailure::Domain(WgError::new(code, "x"));
        assert_eq!(failure.exit_code(), exit, "{code:?}");
    }
    assert_eq!(CliFailure::fail("x", 2).exit_code(), 2);
    assert_eq!(CliFailure::Interrupted.exit_code(), 130);
}

#[test]
fn domain_error_json_is_one_compact_line_on_stdout() {
    let mut error = WgError::new(ErrorCode::NotFound, "task WG-9 not found");
    error.details.insert("ref".to_owned(), json!("WG-9"));
    let out = report(&CliFailure::Domain(error), true, &console());
    assert_eq!(out.code, 4);
    assert!(out.stderr.is_empty());
    assert_eq!(
        out.stdout,
        "{\"error\": {\"code\": \"not_found\", \"message\": \"task WG-9 not found\", \"details\": {\"ref\": \"WG-9\"}}}\n"
    );
}

#[test]
fn domain_error_human_goes_to_stderr_with_details() {
    let mut error = WgError::new(ErrorCode::Conflict, "busy");
    error.details.insert("holder".to_owned(), json!("w1"));
    error.details.insert("errors".to_owned(), json!([1]));
    error.details.insert("n".to_owned(), json!(3));
    let out = report(&CliFailure::Domain(error), false, &console());
    assert_eq!(out.code, 3);
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, "error (conflict): busy\n  holder: w1\n  n: 3\n");
}

#[test]
fn fail_uses_the_cli_error_code() {
    let json = report(&CliFailure::fail("bad thing", 2), true, &console());
    assert_eq!(json.code, 2);
    assert_eq!(
        json.stdout,
        "{\"error\": {\"code\": \"cli_error\", \"message\": \"bad thing\", \"details\": {}}}\n"
    );
    let human = report(&CliFailure::fail("bad thing", 2), false, &console());
    assert_eq!(human.stderr, "error: bad thing\n");
    assert!(human.stdout.is_empty());
    let with_details = CliFailure::Fail {
        message: "m".to_owned(),
        code: 1,
        details: Map::from_iter([("k".to_owned(), json!(1))]),
    };
    assert!(
        report(&with_details, true, &console())
            .stdout
            .contains("\"details\": {\"k\": 1}")
    );
}

#[test]
fn exit_and_interrupt_print_nothing() {
    for failure in [CliFailure::Exit(3), CliFailure::Interrupted] {
        let out = report(&failure, true, &console());
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }
}

#[test]
fn snapshot_parses_the_contract_file() {
    let text = include_str!("../../../contracts/cli.json");
    let snapshot = Snapshot::parse(text).expect("parses");
    assert_eq!(snapshot.commands.len(), 192);
    assert_eq!(snapshot.top_level.len(), 32);
    assert_eq!(snapshot.root_options.len(), 2);
    let list = snapshot
        .commands
        .iter()
        .find(|c| c.path == ["task", "list"])
        .expect("task list");
    assert!(
        list.params
            .iter()
            .any(|p| p.kind == Kind::Option && p.multiple)
    );
    assert!(Snapshot::parse("{}").is_err());
}
