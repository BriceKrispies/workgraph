//! Click's `int` and `float` checks: what Python's `int()` and `float()` read, and the usage error
//! for a value they do not (`Invalid value for '--x': 'v' is not a valid integer.`, exit 2).

#![allow(clippy::expect_used)]

use wg_clispec::{CliFailure, Usage, python_number_text};

const LIST: Usage = Usage {
    usage: "workgraph task list [OPTIONS]",
    command: "workgraph task list",
};

// covers: O05
#[test]
fn python_reads_surrounding_space_and_single_underscores_between_digits() {
    let read = |text: &str| python_number_text(text);
    assert_eq!(read(" 12 ").as_deref(), Some("12"));
    assert_eq!(read("+3").as_deref(), Some("+3"));
    assert_eq!(read("1_000").as_deref(), Some("1000"));
    assert_eq!(read("1.5_5").as_deref(), Some("1.55"));
    for misplaced in ["_5", "5_", "1__0", "1_.5", "1._5", "1_e5", "x_1"] {
        assert_eq!(read(misplaced), None, "{misplaced:?}");
    }
}

// covers: O05
#[test]
fn a_bad_int_is_a_click_usage_error_quoted_as_python_repr() {
    let failure = LIST.invalid_value("--limit", "abc", "integer");
    assert_eq!(
        failure,
        CliFailure::Usage {
            usage: "workgraph task list [OPTIONS]".to_owned(),
            command: "workgraph task list".to_owned(),
            message: "Invalid value for '--limit': 'abc' is not a valid integer.".to_owned(),
        }
    );
    assert_eq!(failure.exit_code(), 2);
    // Python's repr switches to double quotes for a value holding a single quote.
    let CliFailure::Usage { message, .. } = LIST.invalid_value("--limit", "it's", "integer") else {
        panic!("a usage error");
    };
    assert_eq!(
        message,
        "Invalid value for '--limit': \"it's\" is not a valid integer."
    );
}
