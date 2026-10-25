//! Behavioural checks for wg-types against the Python contracts: O05 exit codes, HTTP status
//! classes, permission defaults, task keys, actors and the 23 enums.

use std::error::Error;

use wg_types::{
    Actor, ActorKind, AttemptStatus, ErrorCode, Permission, TaskKey, TaskStatus, Timestamp,
    TrialOutcome, WgError, default_agent_permissions, parse_permissions,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn error_codes_map_to_exit_codes_o05() {
    let expected = [
        (ErrorCode::WorkgraphError, 1),
        (ErrorCode::GitError, 1),
        (ErrorCode::ValidationFailed, 2),
        (ErrorCode::DependencyCycle, 2),
        (ErrorCode::Conflict, 3),
        (ErrorCode::VersionConflict, 3),
        (ErrorCode::InvalidTransition, 3),
        (ErrorCode::ResourceBusy, 3),
        (ErrorCode::NotFound, 4),
        (ErrorCode::PermissionDenied, 5),
        (ErrorCode::ProviderUnavailable, 6),
        (ErrorCode::ConfigError, 7),
    ];
    assert_eq!(expected.len(), ErrorCode::ALL.len());
    for (code, exit) in expected {
        assert_eq!(code.exit_code(), exit, "exit code of {code}");
    }
}

#[test]
fn error_codes_map_to_http_status() {
    let expected = [
        (ErrorCode::WorkgraphError, 400),
        (ErrorCode::ConfigError, 400),
        (ErrorCode::NotFound, 404),
        (ErrorCode::ValidationFailed, 422),
        (ErrorCode::DependencyCycle, 422),
        (ErrorCode::Conflict, 409),
        (ErrorCode::VersionConflict, 409),
        (ErrorCode::InvalidTransition, 409),
        (ErrorCode::ResourceBusy, 409),
        (ErrorCode::PermissionDenied, 403),
        (ErrorCode::ProviderUnavailable, 503),
        (ErrorCode::GitError, 500),
    ];
    for (code, status) in expected {
        assert_eq!(code.http_status(), status, "status of {code}");
    }
}

#[test]
fn error_code_wire_strings_round_trip() -> TestResult {
    for code in ErrorCode::ALL {
        assert_eq!(code.as_str().parse::<ErrorCode>()?, *code);
    }
    assert!("no_such_code".parse::<ErrorCode>().is_err());
    Ok(())
}

#[test]
fn wg_error_keeps_details_in_insertion_order() {
    let error = WgError::new(ErrorCode::NotFound, "task not found")
        .with_detail("zeta", serde_json::json!(1))
        .with_detail("alpha", serde_json::json!(2));
    let keys: Vec<&str> = error.details.keys().map(String::as_str).collect();
    assert_eq!(keys, ["zeta", "alpha"]);
    assert_eq!(error.exit_code(), 4);
    assert_eq!(error.http_status(), 404);
}

#[test]
fn enum_counts_and_wire_strings() -> TestResult {
    assert_eq!(TaskStatus::ALL.len(), 10);
    assert_eq!(AttemptStatus::ALL.len(), 9);
    assert_eq!(
        TaskStatus::AwaitingVerification.as_str(),
        "awaiting_verification"
    );
    assert_eq!(TrialOutcome::Passed.as_str(), "pass");
    assert_eq!(TrialOutcome::Failed.as_str(), "fail");
    assert_eq!(AttemptStatus::TimedOut.to_string(), "timed_out");
    assert!("sideways".parse::<TaskStatus>().is_err());
    let json = serde_json::to_string(&TaskStatus::Accepted)?;
    assert_eq!(json, "\"accepted\"");
    let back: TaskStatus = serde_json::from_str(&json)?;
    assert_eq!(back, TaskStatus::Accepted);
    Ok(())
}

#[test]
fn permission_defaults_match_python() -> TestResult {
    let agent = default_agent_permissions();
    assert!(agent.contains(&Permission::TaskWrite));
    assert!(agent.contains(&Permission::TelemetryIngest));
    assert!(!agent.contains(&Permission::TaskAccept));
    assert!(!agent.contains(&Permission::ConfigManage));
    assert!(!agent.contains(&Permission::RunStart));
    assert_eq!(agent.len(), 6 + 8);

    let unknown = parse_permissions(&["task:read", "task:fly"]);
    let error = unknown.err().ok_or("unknown permission was accepted")?;
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    Ok(())
}

#[test]
fn task_keys_validate_and_keep_spelling() -> TestResult {
    let key = TaskKey::parse("WG-007")?;
    assert_eq!(key.prefix(), "WG");
    assert_eq!(key.number(), "007");
    assert_eq!(key.to_string(), "WG-007");
    for bad in ["wg-1", "WG-", "WG1", "-1", "WG-1a", "1WG-1"] {
        assert!(TaskKey::parse(bad).is_err(), "{bad} should be rejected");
    }
    Ok(())
}

#[test]
fn actors_parse_and_display() -> TestResult {
    let actor = Actor::parse("agent:claude-code")?;
    assert_eq!(actor.kind(), ActorKind::Agent);
    assert_eq!(actor.name(), "claude-code");
    assert_eq!(actor.to_string(), "agent:claude-code");
    assert_eq!(Actor::parse("system:a:b")?.name(), "a:b");
    assert!(Actor::parse("robot:x").is_err());
    assert!(Actor::parse("human:").is_err());
    Ok(())
}

#[test]
fn timestamps_keep_microseconds_and_convert_to_utc() -> TestResult {
    let stamp = Timestamp::parse_iso("2026-10-25T15:00:00.123456+02:00")?;
    assert_eq!(stamp.format_isoformat(), "2026-10-25T13:00:00.123456+00:00");
    assert_eq!(stamp.format_db(), "2026-10-25T13:00:00.123456+00:00");
    assert!(
        Timestamp::parse_iso("2026-10-25T13:00:00").is_err(),
        "naive values are rejected"
    );
    assert!(Timestamp::from_components(0, 1, 1, 0, 0, 0, 0).is_err());
    Ok(())
}
