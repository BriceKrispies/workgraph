//! The forecast of a task's declared size from a fit's stored lists, and the repository it names
//! (the pure half of `task contract-check`).

#![allow(clippy::expect_used)]

use serde_json::{Value, json};
use wg_experiments::predictor::{forecast_task, repository_ref};

fn metadata() -> Value {
    json!({"contract": {"predicted": {"src_lines": 1, "src_files": 1, "packages": []}}})
}

#[test]
fn zero_coefficients_predict_one_half_without_an_interval() {
    let coef = [json!(0.0), json!(0.0), json!(0.0), json!(0.0)];
    let forecast = forecast_task(&coef, &[], &[], None, &metadata()).expect("forecast");
    assert_eq!(forecast.probability, 0.5);
    assert_eq!((forecast.low, forecast.high), (None, None));
    assert!(!forecast.repository_known);
}

#[test]
fn the_fitted_repository_adds_its_indicator() {
    // Intercept -1, three feature slopes at 0, and a repository slope of 2: the known repository
    // gives eta = 1, so the probability is sigmoid(1).
    let coef = [json!(-1.0), json!(0.0), json!(0.0), json!(0.0), json!(2.0)];
    let repos = ["repo-a".to_owned()];
    let known = forecast_task(&coef, &[], &repos, Some("repo-a"), &metadata()).expect("known");
    assert!(known.repository_known);
    assert!((known.probability - 0.731_058_578_630_004_9).abs() < 1e-12);
    let unknown = forecast_task(&coef, &[], &repos, Some("repo-b"), &metadata()).expect("unknown");
    assert!(!unknown.repository_known);
    assert!((unknown.probability - 0.268_941_421_369_995_1).abs() < 1e-12);
}

#[test]
fn a_non_numeric_coefficient_is_refused() {
    let coef = [json!(0.0), json!("x")];
    assert!(forecast_task(&coef, &[], &[], None, &metadata()).is_err());
}

#[test]
fn repository_names_follow_the_workspace_lookup() {
    let known = repository_ref(
        Some("0f".to_owned()),
        Some(("core".to_owned(), "0f-id".to_owned())),
    );
    assert_eq!(known.name.as_deref(), Some("core"));
    assert_eq!(known.id.as_deref(), Some("0f-id"));
    let unknown = repository_ref(Some("gone".to_owned()), None);
    assert_eq!(unknown.name.as_deref(), Some("gone"));
    assert_eq!(unknown.id, None);
    let unlinked = repository_ref(None, None);
    assert_eq!((unlinked.name, unlinked.id), (None, None));
}
