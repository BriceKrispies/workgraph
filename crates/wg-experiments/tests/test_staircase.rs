//! Port of `tests/unit/test_staircase.py`: the same test names and cases over
//! `wg_experiments::staircase`.

use std::error::Error;

use wg_experiments::staircase::{
    Lookup, Phase, ProtocolFields, Raw, StaircaseConfig, Step, next_step,
};

type TestResult = Result<(), Box<dyn Error>>;

fn config() -> StaircaseConfig {
    StaircaseConfig {
        min_band: 1,
        max_band: 8,
        per_visit: 2,
        up: 1,
        down: 2,
        max_reversals: 3,
        max_trials: 24,
        refine_trials: 4,
        refine_width: 1,
    }
}

fn step(
    config: &StaircaseConfig,
    staircase: &[(i64, bool)],
    refine: &[(i64, bool)],
) -> Result<Step, Box<dyn Error>> {
    Ok(next_step(config, staircase, refine)?)
}

fn error_of(config: &StaircaseConfig, staircase: &[(i64, bool)], refine: &[(i64, bool)]) -> String {
    match next_step(config, staircase, refine) {
        Ok(_) => String::from("no error"),
        Err(err) => err.message().to_string(),
    }
}

#[test]
fn test_starts_at_top_band() -> TestResult {
    let got = step(&config(), &[], &[])?;
    assert_eq!(got.phase, Phase::Staircase);
    assert_eq!(got.band, Some(8));
    assert_eq!(got.reversals, 0);
    assert_eq!(got.threshold, None);
    assert!(got.visits.is_empty());
    Ok(())
}

#[test]
fn test_failing_visit_steps_down_by_down() -> TestResult {
    let got = step(&config(), &[(8, false), (8, false)], &[])?;
    assert_eq!(got.band, Some(6));
    assert_eq!(got.visits, vec![(8, 0, 2)]);
    assert_eq!(got.threshold, Some(8.0));
    Ok(())
}

#[test]
fn test_one_pass_in_a_visit_counts_as_passed() -> TestResult {
    assert_eq!(
        step(&config(), &[(4, true), (4, false)], &[])?.band,
        Some(5)
    );
    Ok(())
}

#[test]
fn test_passing_visit_steps_up_by_up() -> TestResult {
    let config = StaircaseConfig { up: 2, ..config() };
    assert_eq!(step(&config, &[(3, true), (3, true)], &[])?.band, Some(5));
    Ok(())
}

#[test]
fn test_clamps_at_the_top() -> TestResult {
    assert_eq!(step(&config(), &[(8, true), (8, true)], &[])?.band, Some(8));
    Ok(())
}

#[test]
fn test_clamps_at_the_bottom() -> TestResult {
    assert_eq!(
        step(&config(), &[(2, false), (2, false)], &[])?.band,
        Some(1)
    );
    assert_eq!(
        step(&config(), &[(1, false), (1, false)], &[])?.band,
        Some(1)
    );
    Ok(())
}

#[test]
fn test_incomplete_visit_stays_at_its_band() -> TestResult {
    let got = step(&config(), &[(5, true)], &[])?;
    assert_eq!(got.band, Some(5));
    assert_eq!(got.phase, Phase::Staircase);
    assert!(got.visits.is_empty());
    assert_eq!(got.threshold, None);
    Ok(())
}

#[test]
fn test_incomplete_visit_after_complete_visits_continues_at_its_band() -> TestResult {
    let got = step(&config(), &[(5, true), (5, false), (6, true)], &[])?;
    assert_eq!(got.band, Some(6));
    assert_eq!(got.visits, vec![(5, 1, 2)]);
    Ok(())
}

#[test]
fn test_visit_with_mixed_bands_raises() {
    assert!(error_of(&config(), &[(5, true), (6, false)], &[]).contains("mixes bands"));
}

#[test]
fn test_reversal_is_counted_including_clamped_moves() -> TestResult {
    // Pass at the top is clamped but still an "up" move; the failure after it is a reversal.
    let history = [
        (8, true),
        (8, true),
        (8, false),
        (8, false),
        (6, false),
        (6, false),
    ];
    let got = step(&config(), &history, &[])?;
    assert_eq!(got.reversals, 1);
    assert_eq!(got.threshold, Some(8.0));
    assert_eq!(got.band, Some(4));
    assert_eq!(got.visits, vec![(8, 2, 2), (8, 0, 2), (6, 0, 2)]);
    Ok(())
}

#[test]
fn test_no_reversal_when_direction_is_unchanged() -> TestResult {
    let got = step(
        &config(),
        &[(8, false), (8, false), (6, false), (6, false)],
        &[],
    )?;
    assert_eq!(got.reversals, 0);
    assert_eq!(got.threshold, Some(6.0));
    Ok(())
}

#[test]
fn test_threshold_is_mean_of_reversal_bands() -> TestResult {
    // Down at 8, up at 6 (reversal), down at 7 (reversal): threshold is mean(6, 7).
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let got = step(&config(), &history, &[])?;
    assert_eq!(got.reversals, 2);
    assert_eq!(got.threshold, Some(6.5));
    assert_eq!(got.phase, Phase::Staircase);
    Ok(())
}

#[test]
fn test_threshold_without_reversal_is_last_visit_band() -> TestResult {
    let got = step(
        &config(),
        &[(5, false), (5, false), (3, false), (3, false)],
        &[],
    )?;
    assert_eq!(got.reversals, 0);
    assert_eq!(got.threshold, Some(3.0));
    assert_eq!(got.band, Some(1));
    Ok(())
}

#[test]
fn test_stops_at_max_reversals() -> TestResult {
    // Down at 8, up at 6, down at 7: the third visit makes the second reversal.
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let got = step(
        &StaircaseConfig {
            max_reversals: 2,
            refine_trials: 0,
            ..config()
        },
        &history,
        &[],
    )?;
    assert_eq!(got.phase, Phase::Done);
    assert_eq!(got.band, None);
    assert_eq!(got.reversals, 2);
    Ok(())
}

#[test]
fn test_stops_at_max_trials() -> TestResult {
    let history = [(5, true), (5, true), (6, false), (6, false)];
    let got = step(
        &StaircaseConfig {
            max_trials: 4,
            refine_trials: 0,
            ..config()
        },
        &history,
        &[],
    )?;
    assert_eq!(got.phase, Phase::Done);
    assert_eq!(got.threshold, Some(6.0));
    Ok(())
}

#[test]
fn test_max_trials_ends_the_phase_even_mid_visit() -> TestResult {
    // The fifth trial opens a new visit at band 6; max_trials ends the phase there, and the open visit
    // is not counted in the visits summary.
    let history = [(5, true), (5, true), (6, false), (6, false), (6, true)];
    let got = step(
        &StaircaseConfig {
            max_trials: 5,
            refine_trials: 0,
            ..config()
        },
        &history,
        &[],
    )?;
    assert_eq!(got.phase, Phase::Done);
    assert_eq!(got.visits, vec![(5, 2, 2), (6, 0, 2)]);
    Ok(())
}

#[test]
fn test_reversal_stop_waits_for_visit_boundary() -> TestResult {
    let config = StaircaseConfig {
        max_reversals: 1,
        refine_trials: 0,
        ..config()
    };
    let at_boundary = [(8, false), (8, false), (6, true), (6, true)];
    assert_eq!(step(&config, &at_boundary, &[])?.phase, Phase::Done);
    // One more trial starts a visit: the staircase keeps running at that band.
    let mut longer = at_boundary.to_vec();
    longer.push((7, false));
    let got = step(&config, &longer, &[])?;
    assert_eq!(got.phase, Phase::Staircase);
    assert_eq!(got.band, Some(7));
    Ok(())
}

#[test]
fn test_refine_starts_after_staircase_ends() -> TestResult {
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let got = step(
        &StaircaseConfig {
            max_reversals: 2,
            ..config()
        },
        &history,
        &[],
    )?;
    assert_eq!(got.phase, Phase::Refine);
    assert_eq!(got.threshold, Some(6.5));
    // round(6.5) == 6 and the window is [5, 7]; all counts are zero so the tie goes to the closest band,
    // which is 6 and 7 equally (0.5 away); the lower band wins.
    assert_eq!(got.band, Some(6));
    Ok(())
}

#[test]
fn test_refine_prefers_band_with_fewest_trials() -> TestResult {
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let config = StaircaseConfig {
        max_reversals: 2,
        ..config()
    };
    // Band 6 has one trial, so the untried bands 5 and 7 tie on count; 7 is closer to 6.5.
    assert_eq!(step(&config, &history, &[(6, true)])?.band, Some(7));
    // Bands 5, 6 and 7 are then all tried once and tie on count; 6 and 7 are equally close, so 6 wins.
    assert_eq!(
        step(&config, &history, &[(6, true), (7, false), (5, true)])?.band,
        Some(6)
    );
    // Band 6 has the most trials, so the choice moves to the untried band 5.
    assert_eq!(
        step(&config, &history, &[(6, true), (7, false)])?.band,
        Some(5)
    );
    Ok(())
}

#[test]
fn test_refine_tie_on_count_and_distance_goes_to_lower_band() -> TestResult {
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let config = StaircaseConfig {
        max_reversals: 2,
        ..config()
    };
    // Bands 6 and 7 are both at distance 0.5 from the threshold 6.5 and both have zero trials here.
    assert_eq!(step(&config, &history, &[(5, true)])?.band, Some(6));
    Ok(())
}

#[test]
fn test_refine_is_done_after_its_trials() -> TestResult {
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let refine = [(6, true), (7, false), (5, true), (6, false)];
    let got = step(
        &StaircaseConfig {
            max_reversals: 2,
            ..config()
        },
        &history,
        &refine,
    )?;
    assert_eq!(got.phase, Phase::Done);
    assert_eq!(got.band, None);
    Ok(())
}

#[test]
fn test_refine_window_is_clamped_to_band_range() -> TestResult {
    // Threshold 1.0 with width 2: window is [1, 3] because 1 is the bottom of the range.
    let history = [(2, false), (2, false), (1, true), (1, true)];
    let config = StaircaseConfig {
        max_reversals: 1,
        min_band: 1,
        refine_width: 2,
        ..config()
    };
    let got = step(&config, &history, &[])?;
    assert_eq!(got.threshold, Some(1.0));
    assert_eq!(got.phase, Phase::Refine);
    assert_eq!(got.band, Some(1));
    assert_eq!(step(&config, &history, &[(1, true)])?.band, Some(2));
    Ok(())
}

#[test]
fn test_refine_with_zero_trials_is_done() -> TestResult {
    let history = [
        (8, false),
        (8, false),
        (6, true),
        (6, true),
        (7, false),
        (7, false),
    ];
    let got = step(
        &StaircaseConfig {
            max_reversals: 2,
            refine_trials: 0,
            ..config()
        },
        &history,
        &[],
    )?;
    assert_eq!(got.phase, Phase::Done);
    assert_eq!(got.threshold, Some(6.5));
    Ok(())
}

#[test]
fn test_refine_before_staircase_finishes_raises() {
    assert!(error_of(&config(), &[(8, false)], &[(7, true)]).contains("refine"));
}

fn protocol_fields(
    min: Lookup,
    max: Lookup,
    per_visit: Lookup,
    up: Lookup,
    down: Lookup,
    max_reversals: Lookup,
    max_trials: Lookup,
) -> ProtocolFields {
    ProtocolFields {
        has_bands: true,
        has_staircase: true,
        min,
        max,
        per_visit,
        up,
        down,
        max_reversals,
        max_trials,
        refine_trials: None,
        refine_width: None,
    }
}

fn found(value: i64) -> Lookup {
    Lookup::Found(Raw::Int(value))
}

#[test]
fn test_from_protocol_reads_fields_and_defaults() -> TestResult {
    let protocol = ProtocolFields {
        refine_trials: Some(Raw::Int(16)),
        refine_width: Some(Raw::Int(1)),
        ..protocol_fields(
            found(1),
            found(9),
            found(2),
            found(1),
            found(2),
            found(6),
            found(24),
        )
    };
    let config = StaircaseConfig::from_protocol(&protocol)?;
    assert_eq!(
        config,
        StaircaseConfig {
            min_band: 1,
            max_band: 9,
            per_visit: 2,
            up: 1,
            down: 2,
            max_reversals: 6,
            max_trials: 24,
            refine_trials: 16,
            refine_width: 1,
        }
    );
    let no_refine = protocol_fields(
        found(1),
        found(9),
        found(2),
        found(1),
        found(2),
        found(6),
        found(24),
    );
    let defaulted = StaircaseConfig::from_protocol(&no_refine)?;
    assert_eq!(defaulted.refine_trials, 0);
    assert_eq!(defaulted.refine_width, 1);
    Ok(())
}

#[test]
fn test_from_protocol_names_the_missing_field() {
    let protocol = ProtocolFields {
        down: Lookup::Absent,
        ..protocol_fields(
            found(1),
            found(9),
            found(2),
            found(1),
            Lookup::Absent,
            found(6),
            found(24),
        )
    };
    let err = StaircaseConfig::from_protocol(&protocol)
        .err()
        .map(|e| e.message().to_string());
    assert!(err.unwrap_or_default().contains("down"));
}

#[test]
fn test_from_protocol_names_the_invalid_field() {
    let protocol = protocol_fields(
        found(1),
        found(9),
        found(0),
        found(1),
        found(2),
        found(6),
        found(24),
    );
    let err = StaircaseConfig::from_protocol(&protocol)
        .err()
        .map(|e| e.message().to_string());
    assert_eq!(
        err.as_deref(),
        Some("staircase.per_visit must be >= 1, got 0")
    );
}

#[test]
fn test_config_rejects_min_above_max() {
    let config = StaircaseConfig {
        min_band: 9,
        max_band: 3,
        ..config()
    };
    let err = config.validate().err().map(|e| e.message().to_string());
    assert!(err.unwrap_or_default().contains("min_band"));
}

#[test]
fn test_config_rejects_max_trials_below_one_visit() {
    let config = StaircaseConfig {
        max_trials: 1,
        per_visit: 2,
        ..config()
    };
    let err = config.validate().err().map(|e| e.message().to_string());
    assert!(err.unwrap_or_default().contains("max_trials"));
}
