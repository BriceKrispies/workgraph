//! GV-02 (check selection): every case under `golden/verify/scope.json` must select the same checks,
//! in the same order, for every tier, as `workgraph.verification.configs.applicable_checks` and
//! `checks_for_tier` do in Python. The vectors come from `tools/golden/gen_verify_scope.py`.
#![allow(clippy::expect_used)]

use rpds::VectorSync;
use serde_json::Value;
use wg_model::entity::VerificationConfig;
use wg_types::{Id, Timestamp};
use wg_verify::{CheckTier, TaskScope, applicable_checks, checks_for_tier};

fn load() -> Value {
    let path = format!(
        "{}/../../golden/verify/scope.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("golden file is readable");
    serde_json::from_str(&text).expect("golden file is JSON")
}

fn id(value: &Value) -> Id {
    Id::parse_str(value.as_str().expect("an id string")).expect("a valid id")
}

fn ids(value: &Value) -> Vec<Id> {
    value.as_array().expect("a list").iter().map(id).collect()
}

fn text(value: &Value) -> String {
    value.as_str().expect("a string").to_owned()
}

fn now() -> Timestamp {
    Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a valid timestamp")
}

/// A stored configuration row, as the store returns it.
fn config(case_config: &Value) -> VerificationConfig {
    let checks = case_config["checks"]
        .as_array()
        .expect("checks list")
        .iter()
        .map(|raw| match raw {
            Value::Object(map) => map.clone(),
            other => panic!("expected a check object, got {other}"),
        })
        .collect::<VectorSync<_>>();
    VerificationConfig {
        id: id(&case_config["id"]),
        workspace_id: id(&case_config["workspace"]),
        scope_type: text(&case_config["scope_type"]),
        scope_id: id(&case_config["scope_id"]),
        name: text(&case_config["name"]),
        checks,
        enabled: case_config["enabled"].as_bool().expect("enabled flag"),
        approved_by: "human:test".to_owned(),
        version: 1,
        created_at: now(),
        updated_at: now(),
    }
}

/// One selected check as the golden records it: config id, config name, scope type, scope id,
/// check name, tier, repository and command.
type Selected = (
    Id,
    String,
    String,
    Id,
    String,
    String,
    Option<String>,
    Vec<String>,
);

/// The selection of one case, the fields the golden records.
fn selection(checks: &[&wg_verify::ApplicableCheck]) -> Vec<Selected> {
    checks
        .iter()
        .map(|check| {
            (
                check.config_id,
                check.config_name.clone(),
                check.scope_type.clone(),
                check.scope_id,
                check.spec.name.clone(),
                check.spec.tier.as_str().to_owned(),
                check.spec.repository.clone(),
                check.spec.command.clone(),
            )
        })
        .collect()
}

fn expected(rows: &Value) -> Vec<Selected> {
    rows.as_array()
        .expect("a list of checks")
        .iter()
        .map(|row| {
            (
                id(&row["config_id"]),
                text(&row["config_name"]),
                text(&row["scope_type"]),
                id(&row["scope_id"]),
                text(&row["name"]),
                text(&row["tier"]),
                row["repository"].as_str().map(str::to_owned),
                row["command"]
                    .as_array()
                    .expect("command list")
                    .iter()
                    .map(text)
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn every_scope_case_selects_the_recorded_checks_and_tiers() {
    let golden = load();
    let cases = golden["cases"].as_array().expect("cases list");
    assert!(!cases.is_empty(), "the vector file has cases");
    for case in cases {
        let case_id = text(&case["id"]);
        let configs: Vec<VerificationConfig> = case["configs"]
            .as_array()
            .expect("configs list")
            .iter()
            .map(config)
            .collect();
        let scope = TaskScope {
            workspace_id: id(&golden["workspace"]),
            task_id: id(&golden["task"]),
            project_ids: ids(&case["task_projects"]),
            repository_ids: ids(&case["task_repositories"]),
        };
        let checks = applicable_checks(&configs, &scope)
            .unwrap_or_else(|err| panic!("case {case_id}: {err}"));
        let all: Vec<&_> = checks.iter().collect();
        assert_eq!(
            selection(&all),
            expected(&case["expected"]["all"]),
            "case {case_id}, all tiers"
        );
        for (tier, key) in [(CheckTier::Fast, "fast"), (CheckTier::Full, "full")] {
            let kept = checks_for_tier(&checks, Some(tier));
            assert_eq!(
                selection(&kept),
                expected(&case["expected"][key]),
                "case {case_id}, tier {key}"
            );
        }
    }
}
