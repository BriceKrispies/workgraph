//! Click's `Choice`, `Path` and typed-option checks as usage errors: the message text, the stream
//! they go to with and without `--json`, and the exit code (contracts O02 and O05).

#![allow(clippy::expect_used)]

use clap::{Arg, Command};
use wg_clispec::args::{float_value, number};
use wg_clispec::{CliFailure, PathKind, PathProblem, Usage, report};
use wg_render::Console;

const LIST: Usage = Usage {
    usage: "workgraph task list [OPTIONS]",
    command: "workgraph task list",
};

/// A clap command with the one option `--name`, parsed from `args`.
fn matches(args: &[&str]) -> clap::ArgMatches {
    Command::new("workgraph")
        .arg(Arg::new("--limit").long("limit").num_args(1))
        .arg(
            Arg::new("--min-confidence")
                .long("min-confidence")
                .num_args(1),
        )
        .try_get_matches_from(std::iter::once("workgraph").chain(args.iter().copied()))
        .expect("the arguments parse")
}

fn usage_message(failure: &CliFailure) -> &str {
    let CliFailure::Usage { message, .. } = failure else {
        panic!("a usage error, not {failure:?}");
    };
    message
}

// covers: O05
#[test]
fn an_int_that_python_cannot_read_is_click_s_integer_usage_error() {
    let m = matches(&["--limit", "abc"]);
    let failure = number(&m, "--limit", 50, LIST).expect_err("abc is not an integer");
    assert_eq!(
        usage_message(&failure),
        "Invalid value for '--limit': 'abc' is not a valid int."
    );
    assert_eq!(failure.exit_code(), 2);

    // Python's int() reads surrounding space and single underscores between digits.
    let m = matches(&["--limit", " 1_000 "]);
    assert_eq!(number(&m, "--limit", 50, LIST).ok(), Some(1000));
    let m = matches(&[]);
    assert_eq!(number(&m, "--limit", 50, LIST).ok(), Some(50));
}

// covers: O05
#[test]
fn a_float_that_python_cannot_read_is_click_s_float_usage_error() {
    let m = matches(&["--min-confidence", "high"]);
    let failure = float_value(&m, "--min-confidence", LIST).expect_err("high is not a float");
    assert_eq!(
        usage_message(&failure),
        "Invalid value for '--min-confidence': 'high' is not a valid float."
    );
    let m = matches(&["--min-confidence", "0.75"]);
    assert_eq!(
        float_value(&m, "--min-confidence", LIST).ok(),
        Some(Some(0.75))
    );
}

// covers: O05
#[test]
fn a_choice_outside_its_choices_lists_them_as_click_does() {
    let failure = LIST.invalid_choice("--status", "wip", &["open", "done"]);
    assert_eq!(
        usage_message(&failure),
        "Invalid value for '--status': 'wip' is not one of 'open', 'done'."
    );
    let single = LIST.invalid_choice("--mode", "x", &["fast"]);
    assert_eq!(
        usage_message(&single),
        "Invalid value for '--mode': 'x' is not 'fast'."
    );
    // Python's repr switches quotes for a value holding a single quote.
    let quoted = LIST.invalid_choice("--status", "it's", &["open"]);
    assert_eq!(
        usage_message(&quoted),
        "Invalid value for '--status': \"it's\" is not 'open'."
    );
}

// covers: O05
#[test]
fn a_missing_file_argument_is_click_s_path_usage_error() {
    let failure = LIST.invalid_path("path", "missing.json", PathKind::File, PathProblem::Missing);
    assert_eq!(
        usage_message(&failure),
        "Invalid value for 'path': File 'missing.json' does not exist."
    );
    let directory = LIST.invalid_path("--root", "src", PathKind::Directory, PathProblem::IsFile);
    assert_eq!(
        usage_message(&directory),
        "Invalid value for '--root': Directory 'src' is a file."
    );
    let unreadable = LIST.invalid_path(
        "path",
        "locked.json",
        PathKind::File,
        PathProblem::Unreadable,
    );
    assert_eq!(
        usage_message(&unreadable),
        "Invalid value for 'path': File 'locked.json' is not readable."
    );
    let either = LIST.invalid_path("--where", "dir", PathKind::Path, PathProblem::IsDirectory);
    assert_eq!(
        usage_message(&either),
        "Invalid value for '--where': Path 'dir' is a directory."
    );
}

// covers: O02
#[test]
fn a_usage_error_goes_to_standard_error_as_text_even_with_json() {
    let failure = LIST.invalid_value("--limit", "abc", "int");
    let console = Console::new(80);
    for json in [false, true] {
        let printed = report(&failure, json, &console);
        assert_eq!(
            printed.stdout, "",
            "json={json}: nothing on standard output"
        );
        let lines: Vec<&str> = printed.stderr.lines().collect();
        assert_eq!(
            lines[0], "Usage: workgraph task list [OPTIONS]",
            "json={json}"
        );
        assert_eq!(
            lines[1], "Try 'workgraph task list --help' for help.",
            "json={json}"
        );
        assert!(
            lines[2].starts_with("+- Error "),
            "json={json}: {}",
            lines[2]
        );
        assert!(
            lines[3].contains("Invalid value for '--limit': 'abc' is not a valid int."),
            "json={json}: {}",
            lines[3]
        );
        assert!(
            lines[4].starts_with('+') && lines[4].ends_with('+'),
            "json={json}"
        );
        assert_eq!(printed.code, 2, "json={json}");
    }
}
