//! Workspace settings, allowed roots and path boundaries: the case of `tests/unit/test_foundations.py`
//! that covers path boundaries (`test_path_boundaries`), and the allowed-roots and settings rules of
//! the workspace and repository services.

use std::fmt::Debug;

use serde_json::{Value, json};
use wg_model::JsonObject;
use wg_registry::boundary::{check_allowed_roots, is_within, require_within, safe_relative_path};
use wg_registry::settings::{ALLOWED_SETTINGS, merge, validate_update};
use wg_types::{ErrorCode, WgError};

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("expected success, got {err:?}"),
    }
}

fn err<T: Debug>(result: Result<T, WgError>) -> WgError {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(err) => err,
    }
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("invalid fixture: {other}"),
    }
}

#[test]
fn safe_relative_path_rejects_traversal_and_anchors() {
    let rejected = err(safe_relative_path("../etc/passwd"));
    assert_eq!(rejected.code, ErrorCode::ValidationFailed);
    assert_eq!(
        rejected.message,
        "path '../etc/passwd' must be relative and must not traverse upwards"
    );
    assert!(safe_relative_path("/abs").is_err());
    assert!(safe_relative_path("C:/abs").is_err());
    assert!(safe_relative_path("a/../b").is_err());
    assert!(safe_relative_path("").is_err());
    assert!(safe_relative_path("a\0b").is_err());
    assert_eq!(ok(safe_relative_path("a/b.txt")), "a/b.txt");
    assert_eq!(ok(safe_relative_path("a/./b//c")), "a/b/c");
    assert_eq!(ok(safe_relative_path("a\\b.txt")), "a/b.txt");
}

#[test]
fn require_within_matches_python_boundaries() {
    let root = "/work/space";
    assert_eq!(
        ok(require_within("/work/space/a/b", root)),
        "/work/space/a/b"
    );
    assert_eq!(ok(require_within("/work/space", root)), "/work/space");
    // `tmp_path / ".." / "x"` lies outside the boundary once the `..` is resolved.
    let escaped = err(require_within("/work/space/../x", root));
    assert_eq!(escaped.code, ErrorCode::ValidationFailed);
    assert_eq!(
        escaped
            .details
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["path", "boundary"]
    );
}

#[test]
fn containment_is_by_component_not_by_prefix() {
    assert!(is_within("/work/space/a", "/work/space"));
    assert!(is_within("/work/space", "/work/space"));
    assert!(!is_within("/work/spacex", "/work/space"));
    assert!(!is_within("/work", "/work/space"));
    assert!(!is_within("work/space/a", "/work/space"));
    assert!(is_within("C:\\Repos\\app", "c:/repos"));
    assert!(is_within("C:/Repos/app/../other", "C:/Repos"));
    assert!(!is_within("C:/Repos/../other", "C:/Repos"));
}

#[test]
fn allowed_roots_absent_or_empty_permit_every_path() {
    assert!(check_allowed_roots("/anywhere", &JsonObject::new()).is_ok());
    assert!(check_allowed_roots("/anywhere", &object(json!({"allowed_roots": []}))).is_ok());
}

#[test]
fn allowed_roots_restrict_registration_to_beneath_a_root() {
    let settings = object(json!({"allowed_roots": ["/srv/repos", "/home/me/code"]}));
    assert!(check_allowed_roots("/srv/repos/app", &settings).is_ok());
    assert!(check_allowed_roots("/home/me/code", &settings).is_ok());
    let outside = err(check_allowed_roots("/tmp/app", &settings));
    assert_eq!(outside.code, ErrorCode::ValidationFailed);
    assert_eq!(
        outside.message,
        "/tmp/app is outside the workspace's allowed roots"
    );
    assert_eq!(
        outside.details.get("allowed_roots"),
        Some(&json!(["/srv/repos", "/home/me/code"]))
    );
}

#[test]
fn settings_keys_are_checked_sorted_and_roots_must_be_strings() {
    assert_eq!(
        ALLOWED_SETTINGS,
        [
            "allowed_roots",
            "default_project",
            "description_classification"
        ]
    );
    assert!(
        validate_update(&object(
            json!({"allowed_roots": ["/a"], "default_project": "p"})
        ))
        .is_ok()
    );

    let unknown = err(validate_update(&object(json!({
        "zeta": 1,
        "alpha": 2,
        "allowed_roots": []
    }))));
    assert_eq!(unknown.code, ErrorCode::ValidationFailed);
    assert_eq!(
        unknown.message,
        "unknown workspace settings: ['alpha', 'zeta']"
    );

    for bad in [json!("/a"), json!([1]), json!(null)] {
        let rejected = err(validate_update(&object(json!({ "allowed_roots": bad }))));
        assert_eq!(
            rejected.message,
            "allowed_roots must be a list of directory paths"
        );
    }
}

#[test]
fn settings_merge_overwrites_named_keys_and_keeps_the_rest() {
    let before = object(json!({"default_project": "old", "allowed_roots": ["/a"]}));
    let update = object(json!({"default_project": "new"}));
    assert_eq!(
        merge(&before, &update),
        object(json!({"default_project": "new", "allowed_roots": ["/a"]}))
    );
    assert_eq!(merge(&JsonObject::new(), &update), update);
}
