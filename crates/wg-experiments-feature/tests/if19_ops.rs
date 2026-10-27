//! IF-19: the pure pieces of `experiment create, next, report, fit, predict`: protocol
//! validation, the predictor fit and the prediction read back from it.

use serde_json::{Value, json};
use wg_experiments_feature::spec::predict::{
    FitRow, FitSettings, PredictOptions, PredictRequest, fit, predict_request, prediction,
    select_fit,
};
use wg_experiments_feature::spec::protocol::{protocol_problems, validate_protocol};
use wg_model::{Row, Stamp, Write, derive_id};
use wg_types::{Actor, ActorKind, ErrorCode, Id, Permission, Timestamp, WgError};

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("unexpected failure: {err}"),
    }
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn stamp() -> Stamp {
    Stamp {
        at: ok(Timestamp::from_components(2026, 10, 26, 12, 0, 0, 0)),
        entropy: entropy(7),
        actor: ok(Actor::new(ActorKind::Human, "brice")),
        permissions: [Permission::ExperimentWrite].into_iter().collect(),
        correlation: derive_id(&entropy(99), 0).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn id(n: u32) -> Id {
    derive_id(&entropy(3), n)
}

fn valid() -> Value {
    json!({
        "arms": [{"name": "a", "model": "claude-haiku-5-5"}],
        "brief_template": "b",
        "primary_size": "src_lines",
        "bands": {"min": 1, "max": 5},
        "staircase": {"per_visit": 1, "up": 1, "down": 1, "max_reversals": 2, "max_trials": 4},
        "budget": {"trials": 3},
    })
}

// covers: C069
#[test]
fn protocol_problems_are_reported_together_in_the_python_order() {
    assert!(protocol_problems(&valid()).is_empty());
    assert_eq!(
        protocol_problems(&json!([])),
        ["protocol must be a JSON object"]
    );
    let mut broken = valid();
    broken["bands"] = json!({"min": 5, "max": 1});
    broken["staircase"] =
        json!({"per_visit": 0, "up": 1, "down": 1, "max_reversals": 2, "max_trials": 4, "x": 1});
    broken["arms"] = json!([{"name": "a", "model": "opus"}, {"name": "a", "model": "claude"}]);
    broken["extra"] = json!(1);
    broken["clone"] = json!("copy");
    assert_eq!(
        protocol_problems(&broken),
        [
            "unknown keys: extra".to_owned(),
            "arms[0].model is the alias 'opus'; pin a full model id such as 'claude-haiku-5-5'"
                .to_owned(),
            "arms[1].name 'a' is used by another arm".to_owned(),
            "arms[1].model is not a pinned model id (expected a full id such as 'claude-haiku-5-5')"
                .to_owned(),
            "bands.min must not exceed bands.max".to_owned(),
            "staircase.per_visit must be an integer >= 1".to_owned(),
            "staircase has unknown keys: x".to_owned(),
            "clone must be one of: shared, isolated".to_owned(),
        ]
    );
    let error = validate_protocol(&json!({"arms": 3}));
    assert!(error.is_err_and(|e| e.code == ErrorCode::ValidationFailed));
}

fn rows() -> Vec<FitRow> {
    // Larger leaves fail; a few small ones fail and large ones pass so the fit is not separable.
    let sizes = [
        (1.0, true),
        (2.0, true),
        (3.0, true),
        (4.0, false),
        (3.5, false),
        (2.5, true),
        (5.0, false),
        (1.5, true),
        (4.5, true),
        (0.5, false),
    ];
    sizes
        .iter()
        .enumerate()
        .map(|(n, (log2_lines, passed))| FitRow {
            trial_id: id(n as u32),
            repository: (n % 2 == 0).then(|| id(100)),
            features: vec![*log2_lines, 1.0, 2.0],
            passed: *passed,
            graded_at: Some(ok(Timestamp::from_components(
                2026, 10, 20, 9, 0, n as u8, 0,
            ))),
        })
        .collect()
}

fn settings(samples: i64) -> FitSettings {
    FitSettings {
        repo_penalty: 1.0,
        feature_penalty: 0.01,
        samples,
        seed: 5,
    }
}

// covers: C070, C074
#[test]
fn a_fit_stores_its_layout_and_a_prediction_reads_it_back() {
    let workspace = id(1);
    let decision = ok(fit(
        &stamp(),
        workspace,
        "claude-haiku-5-5",
        &rows(),
        &settings(40),
    ));
    let record = decision.reply.clone();
    assert_eq!(record.n, 10);
    assert_eq!(record.coef.len(), 5);
    assert_eq!(record.feature_names.len(), 4);
    assert_eq!(record.feature_names[3], format!("repository:{}", id(100)));
    assert_eq!(record.metrics["n_pass"], json!(6));
    assert_eq!(record.metrics["n_fail"], json!(4));
    assert!(!record.samples.is_empty() && record.samples.len() <= 40);
    assert_eq!(decision.writes.len(), 1);
    assert!(matches!(
        decision.writes.iter().next(),
        Some(Write::Insert(Row::PredictorFit(_)))
    ));
    assert_eq!(decision.events.len(), 1);

    // The same inputs and seed fit the same model.
    let again = ok(fit(
        &stamp(),
        workspace,
        "claude-haiku-5-5",
        &rows(),
        &settings(40),
    ));
    assert_eq!(again.reply.coef, record.coef);
    assert_eq!(again.reply.samples, record.samples);

    let known = (Some("repo".to_owned()), Some(id(100)));
    let shared = (Some("other".to_owned()), None);
    let predicted = ok(prediction(&record, &known, 8.0, 2.0, 2));
    assert!((0.0..=1.0).contains(&predicted.probability));
    assert!(predicted.repository_known);
    assert!(predicted.low.is_some() && predicted.high.is_some());
    let unknown = ok(prediction(&record, &shared, 8.0, 2.0, 2));
    assert!(!unknown.repository_known);
    assert_eq!(unknown.repository.as_deref(), Some("other"));

    // Too few bootstrap samples give no interval.
    let thin = ok(fit(
        &stamp(),
        workspace,
        "claude-haiku-5-5",
        &rows(),
        &settings(5),
    ))
    .reply;
    assert!(ok(prediction(&thin, &known, 8.0, 2.0, 2)).low.is_none());
}

// covers: C070
#[test]
fn a_fit_refuses_too_few_rows_and_a_single_outcome() {
    let few: Vec<FitRow> = rows().into_iter().take(3).collect();
    let error = fit(&stamp(), id(1), "m", &few, &settings(10)).err();
    assert_eq!(
        error.as_ref().map(|e| e.message.as_str()),
        Some(
            "the predictor for m needs at least 4 graded trials with src_lines, src_files and \
             packages; has 3"
        )
    );
    let passes: Vec<FitRow> = rows().into_iter().filter(|row| row.passed).collect();
    let error = fit(&stamp(), id(1), "m", &passes, &settings(10)).err();
    assert_eq!(
        error.as_ref().map(|e| e.message.as_str()),
        Some("the predictor for m needs both pass and fail outcomes; has only passes")
    );
}

// covers: C074
#[test]
fn a_prediction_picks_the_latest_fit_or_the_one_named() {
    let record = ok(fit(&stamp(), id(1), "m", &rows(), &settings(0))).reply;
    let latest = ok(select_fit("m", id(1), None, None, Some(record.clone())));
    assert_eq!(latest.id, record.id);
    let missing = select_fit("m", id(1), None, None, None).err();
    assert!(missing.is_some_and(|e| e.code == ErrorCode::NotFound));
    let wrong_model = select_fit("other", id(1), Some("x"), Some(record.clone()), None).err();
    assert!(wrong_model.is_some_and(|e| e.code == ErrorCode::ValidationFailed));
    let elsewhere = select_fit("m", id(2), Some("x"), Some(record), None).err();
    assert!(elsewhere.is_some_and(|e| e.code == ErrorCode::NotFound));
}

// covers: C074
#[test]
fn predict_options_take_a_task_or_all_of_the_sizes() {
    let sizes = PredictOptions {
        task: None,
        repo: Some("r".to_owned()),
        lines: Some(10.0),
        files: Some(2.0),
        packages: Some("a, ,b,".to_owned()),
    };
    assert_eq!(
        predict_request(&sizes).ok(),
        Some(PredictRequest::Sizes {
            repository: "r".to_owned(),
            src_lines: 10.0,
            src_files: 2.0,
            packages: 2,
        })
    );
    let both = PredictOptions {
        task: Some("WG-1".to_owned()),
        ..sizes.clone()
    };
    assert!(predict_request(&both).is_err());
    let partial = PredictOptions {
        files: None,
        packages: None,
        ..sizes
    };
    let message = predict_request(&partial).err().map(|e| e.message);
    assert_eq!(
        message.as_deref(),
        Some(
            "predict needs --task or all of --repo, --lines, --files, --packages; missing --files, --packages"
        )
    );
}
