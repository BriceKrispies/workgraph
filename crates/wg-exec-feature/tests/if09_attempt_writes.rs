//! IF-09: the pure helpers of the attempt writes and the agent group.

use serde_json::json;
use wg_exec::manifest::Violation;
use wg_exec_feature::spec::attempt_capture::violations_text;
use wg_exec_feature::spec::attempt_writes::{attempt_limit, validation};
use wg_exec_feature::spec::cli_agent::unknown_placeholder;
use wg_types::ErrorCode;

// covers: C022
#[test]
fn violations_print_as_python_prints_a_list_of_dicts() {
    assert_eq!(violations_text(&[]), None);
    let one = Violation {
        repository: "api".to_owned(),
        commits: 1,
        uncommitted_files: 0,
    };
    assert_eq!(
        violations_text(&[one]).as_deref(),
        Some("[{'repository': 'api', 'commits': 1, 'uncommitted_files': 0}]")
    );
}

// covers: C022
#[test]
fn the_attempt_limit_defaults_to_three() {
    let empty = json!({}).as_object().cloned().unwrap_or_default();
    assert_eq!(attempt_limit(&empty), 3);
    let set = json!({"execution": {"max_attempts_per_task": 5}})
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert_eq!(attempt_limit(&set), 5);
}

// covers: C022
#[test]
fn an_unknown_placeholder_names_the_allowed_ones_sorted() {
    let err = unknown_placeholder("nope");
    assert_eq!(err.code, ErrorCode::ConfigError);
    assert_eq!(
        err.message,
        "unknown placeholder {nope} in agent argument template"
    );
    assert_eq!(validation("x").code, ErrorCode::ValidationFailed);
}
