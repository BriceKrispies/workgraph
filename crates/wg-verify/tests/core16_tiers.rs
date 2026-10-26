//! Port of `tests/unit/test_verification_tiers.py`, plus the config validation those tests lean on.
#![allow(clippy::expect_used)]

use serde_json::{Map, Value, json};
use wg_model::entity::JsonObject;
use wg_types::{Id, WgError};
use wg_verify::{ApplicableCheck, CheckSpec, CheckTier, checks_for_tier, parse_tier};

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("expected an object, got {other}"),
    }
}

fn spec(raw: Value) -> Result<CheckSpec, WgError> {
    CheckSpec::from_json(&object(raw))
}

fn applicable(name: &str, tier: Option<&str>) -> ApplicableCheck {
    let mut raw: Map<String, Value> = object(json!({"name": name, "command": ["pytest"]}));
    if let Some(tier) = tier {
        raw.insert("tier".into(), json!(tier));
    }
    ApplicableCheck {
        config_id: Id::nil(),
        config_name: "ci".into(),
        scope_type: "workspace".into(),
        scope_id: Id::nil(),
        spec: CheckSpec::from_json(&raw).expect("valid check"),
    }
}

fn names(checks: &[&ApplicableCheck]) -> Vec<String> {
    checks.iter().map(|c| c.spec.name.clone()).collect()
}

#[test]
fn check_spec_tier_defaults_to_full() {
    let check = spec(json!({"name": "tests", "command": ["pytest"]})).expect("valid");
    assert_eq!(check.tier, CheckTier::Full);
}

#[test]
fn stored_config_without_tier_loads_as_full() {
    let stored =
        json!({"name": "tests", "kind": "test", "command": ["pytest", "-q"], "required": true});
    assert_eq!(spec(stored).expect("valid").tier, CheckTier::Full);
}

#[test]
fn check_spec_parses_explicit_tier_and_rejects_unknown_values() {
    let fast = spec(json!({"name": "lint", "command": ["ruff", "check"], "tier": "fast"}));
    assert_eq!(fast.expect("valid").tier, CheckTier::Fast);
    assert!(spec(json!({"name": "lint", "command": ["ruff", "check"], "tier": "slow"})).is_err());
    assert!(spec(json!({"name": "lint", "command": ["ruff"], "tiers": "fast"})).is_err());
}

#[test]
fn checks_for_tier_filters_preserving_order() {
    let checks = [
        applicable("lint", Some("fast")),
        applicable("unit", None),
        applicable("types", Some("fast")),
    ];
    assert_eq!(
        names(&checks_for_tier(&checks, Some(CheckTier::Fast))),
        ["lint", "types"]
    );
    assert_eq!(
        names(&checks_for_tier(&checks, Some(CheckTier::Full))),
        ["unit"]
    );
    assert_eq!(
        names(&checks_for_tier(&checks, None)),
        ["lint", "unit", "types"]
    );
    assert!(checks_for_tier(&[], Some(CheckTier::Fast)).is_empty());
}

#[test]
fn parse_tier_accepts_known_tiers_and_none() {
    assert_eq!(parse_tier(None).expect("none is valid"), None);
    assert_eq!(
        parse_tier(Some("fast")).expect("valid"),
        Some(CheckTier::Fast)
    );
    assert_eq!(
        parse_tier(Some("full")).expect("valid"),
        Some(CheckTier::Full)
    );
    assert!(parse_tier(Some("slow")).is_err());
}

#[test]
fn shell_executables_need_allow_shell() {
    let refused = spec(json!({"name": "s", "command": ["C:\\tools\\CMD.exe", "/c", "x"]}));
    assert!(refused.is_err());
    let allowed = spec(json!({"name": "s", "command": ["bash", "-c", "x"], "allow_shell": true}));
    assert!(allowed.is_ok());
}

#[test]
fn env_names_follow_the_python_pattern() {
    assert!(spec(json!({"name": "e", "command": ["x"], "env": {"CI_1": "1"}})).is_ok());
    assert!(spec(json!({"name": "e", "command": ["x"], "env": {"ci": "1"}})).is_err());
    assert!(spec(json!({"name": "e", "command": ["x"], "env": {"1CI": "1"}})).is_err());
}

#[test]
fn timeout_must_be_positive_and_at_most_a_day() {
    assert!(spec(json!({"name": "t", "command": ["x"], "timeout_seconds": 0})).is_err());
    assert!(spec(json!({"name": "t", "command": ["x"], "timeout_seconds": 86401})).is_err());
    let ok = spec(json!({"name": "t", "command": ["x"], "timeout_seconds": 86400}));
    assert!(ok.is_ok());
}

#[test]
fn stored_form_round_trips_through_to_json() {
    let check = spec(json!({
        "name": "unit",
        "command": ["cargo", "test"],
        "parser": "cargo",
        "baseline": "regression",
        "error_exit_codes": [101],
        "tier": "fast",
    }))
    .expect("valid");
    let again = CheckSpec::from_json(&check.to_json()).expect("re-reads");
    assert_eq!(again, check);
}
