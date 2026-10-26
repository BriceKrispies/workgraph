//! `task create --due` takes Click's `datetime` formats and rejects anything else with Click's
//! usage error (exit 2, the message in a panel after the usage line).

#![allow(clippy::expect_used)]

use wg_clispec::datetime::parse_click_datetime;
use wg_clispec::task::create_body;
use wg_clispec::{CliFailure, report};
use wg_render::Console;

fn due(value: &str) -> Option<String> {
    parse_click_datetime(value)
        .ok()
        .map(|naive| wg_json::isoformat(&naive))
}

// covers: C152
#[test]
fn accepted_formats_give_the_naive_datetime_python_stores() {
    let cases = [
        ("2026-10-26", "2026-10-26T00:00:00"),
        ("2026-10-26T10:00:00", "2026-10-26T10:00:00"),
        ("2026-10-26 10:00:00", "2026-10-26T10:00:00"),
        ("2026-1-5", "2026-01-05T00:00:00"),
        ("2026-1-5 1:2:3", "2026-01-05T01:02:03"),
        ("2026-10-26t10:00:00", "2026-10-26T10:00:00"),
        ("2026-10-26  10:00:00", "2026-10-26T10:00:00"),
        ("2026-10-26\t10:00:00", "2026-10-26T10:00:00"),
        ("2026-10- 5", "2026-10-05T00:00:00"),
        ("2026-10-26T23:59:6", "2026-10-26T23:59:06"),
        ("0999-01-01", "0999-01-01T00:00:00"),
        ("2024-02-29", "2024-02-29T00:00:00"),
    ];
    for (input, stored) in cases {
        assert_eq!(due(input).as_deref(), Some(stored), "{input:?}");
    }
}

// covers: C152
#[test]
fn rejected_values_are_the_ones_click_rejects() {
    let cases = [
        "2026-10-26T10:00",
        "2026-10-26T10:00:00+02:00",
        "2026-10-26T10:00:00+0200",
        "2026-10-26T10:00:00Z",
        "2026-10-26T10:00:00.5",
        "2026-10-26 10:00:00.5",
        "garbage",
        "",
        " 2026-10-26",
        "2026-10-26 ",
        "2026-10-26T10:00:00 ",
        "26-10-2026",
        "2026-02-30",
        "2026-13-01",
        "2026-10-26T24:00:00",
        "2026-10-26T10:60:00",
        "2026-10-26T23:59:60",
        "2026-10-26T23:59:61",
        "2026-10-26T23:59:65",
        "0000-01-01",
        "20261026",
        "2026-10-26T",
        "2026-10-26T10:00:00\n",
    ];
    for input in cases {
        assert_eq!(due(input), None, "{input:?}");
    }
}

// covers: C152
#[test]
fn a_rejected_value_is_a_click_usage_error() {
    let Err(failure) = parse_click_datetime("garbage") else {
        panic!("garbage must not parse");
    };
    assert_eq!(
        failure,
        "'garbage' does not match the formats '%Y-%m-%d', '%Y-%m-%dT%H:%M:%S', '%Y-%m-%d %H:%M:%S'."
    );
}

// covers: C152
#[test]
fn the_usage_error_is_text_with_the_panel_and_exit_2() {
    let failure = CliFailure::Usage {
        usage: "workgraph task create [OPTIONS] {title}".to_owned(),
        command: "workgraph task create".to_owned(),
        message: "Invalid value for '--due': '2026-10-26T10:00' does not match the formats '%Y-%m-%d', '%Y-%m-%dT%H:%M:%S', '%Y-%m-%d %H:%M:%S'.".to_owned(),
    };
    assert_eq!(failure.exit_code(), 2);
    // Click writes usage errors as text even under `--json`, and on stderr only.
    let printed = report(&failure, true, &Console::new(80));
    assert_eq!(printed.code, 2);
    assert_eq!(printed.stdout, "");
    // The text Python's CLI prints (typer on a legacy Windows console, 80 columns less one).
    assert_eq!(
        printed.stderr,
        "Usage: workgraph task create [OPTIONS] {title}\n\
         Try 'workgraph task create --help' for help.\n\
         +- Error ---------------------------------------------------------------------+\n\
         | Invalid value for '--due': '2026-10-26T10:00' does not match the formats    |\n\
         | '%Y-%m-%d', '%Y-%m-%dT%H:%M:%S', '%Y-%m-%d %H:%M:%S'.                       |\n\
         +-----------------------------------------------------------------------------+\n"
    );
}

// covers: C152
#[test]
fn create_body_rejects_a_bad_due_before_reading_files() {
    let command = wg_clispec::task::command();
    let matches = command
        .try_get_matches_from([
            "task",
            "create",
            "x",
            "--due",
            "garbage",
            "--description",
            "@/no/such/file",
        ])
        .expect("the clap tree accepts the arguments");
    let (name, create) = matches.subcommand().expect("the create subcommand");
    assert_eq!(name, "create");
    let read = |_: &str| -> Result<String, String> { Err("read".to_owned()) };
    let Err(failure) = create_body(create, &read) else {
        panic!("a bad --due must fail");
    };
    assert!(matches!(failure, CliFailure::Usage { .. }));
}
