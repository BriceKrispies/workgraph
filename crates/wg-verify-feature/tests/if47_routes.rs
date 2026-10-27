//! The verification routes' request handling (IF-47, H086 and H090): the `since` filter is read as
//! Python's `datetime.fromisoformat` reads it (a naive value is UTC), and a configuration write
//! refuses a bad check list before it resolves the workspace, as `create_config` does.

use std::error::Error;

use serde_json::json;
use wg_runtime::{App, human_context};
use wg_types::{ErrorCode, Timestamp};
use wg_verify_feature::service::api;
use wg_verify_feature::spec::config_write::ConfigBody;
use wg_verify_feature::spec::routes::parse_since;

type TestResult = Result<(), Box<dyn Error>>;

// covers: H090
#[test]
fn since_reads_the_values_fromisoformat_reads() -> TestResult {
    let cases = [
        ("2024-01-02", "2024-01-02T00:00:00Z"),
        ("2024-01-02T03:04:05", "2024-01-02T03:04:05Z"),
        ("2024-01-02 03:04:05.123", "2024-01-02T03:04:05.123Z"),
        ("2024-01-02T03:04:05.123456Z", "2024-01-02T03:04:05.123456Z"),
        ("2024-01-02T03:04:05.5Z", "2024-01-02T03:04:05.5Z"),
        ("20240102T030405", "2024-01-02T03:04:05Z"),
        ("2024-01-02T03:04+02:00", "2024-01-02T01:04:00Z"),
        ("2024-01-02T03:04:05-05:30", "2024-01-02T08:34:05Z"),
        ("2024-01-02T03:04:05+0130", "2024-01-02T01:34:05Z"),
        ("2024-01-01T00:00:00+00:00", "2024-01-01T00:00:00Z"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_since(input)?,
            Timestamp::parse_iso(expected)?,
            "{input}"
        );
    }
    Ok(())
}

// covers: H090
#[test]
fn since_refuses_what_fromisoformat_refuses() {
    let cases = [
        "",
        "2024-01-02T",
        "2024-1-02",
        "2024-13-01",
        "2024-02-30",
        "0000-01-01",
        "2024-01-02T25:00",
        "2024-01-02T03:04:60",
        "2024-01-02T03:4",
        "2024-01-02T03:04.5",
        "2024-01-02T03:04:05.Z",
        "2024-01-02T03:04:05+24:00",
        "not a date",
    ];
    for input in cases {
        let err = parse_since(input).expect_err(input);
        assert_eq!(err.code, ErrorCode::ValidationFailed, "{input}");
    }
}

// covers: H086
#[test]
fn a_bad_check_list_is_refused_before_the_workspace_is_looked_up() -> TestResult {
    let home = std::env::temp_dir().join(format!("wg-verify-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(&home))?;
    let context = human_context(None)?;

    // Both wrong: no workspace named `no-such-workspace`, and no check at all. Python raises the
    // validation error, not the workspace's not-found, so the same must hold here.
    let empty = ConfigBody {
        scope_type: "workspace".to_owned(),
        scope: None,
        name: "gate".to_owned(),
        checks: Vec::new(),
    };
    let err = api::create_config(&mut app, &context, "no-such-workspace", &empty)
        .expect_err("an empty check list is refused");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    // Both wrong again, with a check that is not an object.
    let malformed = ConfigBody {
        checks: vec![json!("not an object")],
        ..empty
    };
    let err = api::create_config(&mut app, &context, "no-such-workspace", &malformed)
        .expect_err("a malformed check is refused");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    Ok(())
}
