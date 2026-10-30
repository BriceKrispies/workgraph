//! WG-3820: with a protocol whose band range is degenerate (`min == max`), the planner must plan
//! only bands inside the range. The staircase already clamps; the pool filter is what keeps a
//! task from a band outside the range from being drawn.

use std::collections::BTreeSet;
use std::error::Error;

use wg_experiments::planner::{Task, nearest_unused};
use wg_experiments::staircase::{Phase, StaircaseConfig, next_step};

type TestResult = Result<(), Box<dyn Error>>;

fn degenerate() -> StaircaseConfig {
    StaircaseConfig {
        min_band: 4,
        max_band: 4,
        per_visit: 1,
        up: 1,
        down: 1,
        max_reversals: 1,
        max_trials: 6,
        refine_trials: 2,
        refine_width: 1,
    }
}

#[test]
fn degenerate_range_holds_exactly_one_band() {
    let config = degenerate();
    assert!(config.contains(4));
    assert!(!config.contains(3));
    assert!(!config.contains(5));
}

#[test]
fn range_bounds_are_inclusive() {
    let config = StaircaseConfig {
        min_band: 2,
        max_band: 6,
        ..degenerate()
    };
    assert!([2, 3, 6].into_iter().all(|band| config.contains(band)));
    assert!([1, 7].into_iter().all(|band| !config.contains(band)));
}

#[test]
fn staircase_over_a_degenerate_range_never_leaves_it() -> TestResult {
    let config = degenerate();
    // Failing visits move the band down, but the clamp keeps it at the only band.
    let failing = [(4, false), (4, false), (4, false)];
    let got = next_step(&config, &failing, &[])?;
    assert_eq!(got.phase, Phase::Staircase);
    assert_eq!(got.band, Some(4));

    // Passing visits end the staircase on reversal-free history; refine stays on the one band.
    let done_staircase = [
        (4, true),
        (4, false),
        (4, true),
        (4, false),
        (4, true),
        (4, false),
    ];
    let refine = next_step(&config, &done_staircase, &[])?;
    assert_eq!(refine.phase, Phase::Refine);
    assert_eq!(refine.band, Some(4));
    let refine_done = next_step(&config, &done_staircase, &[(4, true), (4, true)])?;
    assert_eq!(refine_done.phase, Phase::Done);
    assert_eq!(refine_done.band, None);
    Ok(())
}

#[test]
fn planner_skips_out_of_range_bands_in_a_degenerate_range() {
    // Band-3 tasks are the nearest unused band to 4 once the band-4 task is used, but band 3 is
    // outside the range. Filtering the pool by the range leaves no task to draw.
    let config = degenerate();
    let pool = [
        Task {
            key: "T-1".to_string(),
            band: 3,
        },
        Task {
            key: "T-2".to_string(),
            band: 3,
        },
        Task {
            key: "T-3".to_string(),
            band: 4,
        },
    ];
    let in_range: Vec<Task> = pool
        .iter()
        .filter(|task| config.contains(task.band))
        .cloned()
        .collect();

    let used = BTreeSet::new();
    let (chosen, tasks) = nearest_unused(&in_range, &used, 4).expect("band 4 has a task");
    assert_eq!(chosen, 4);
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.key.as_str())
            .collect::<Vec<_>>(),
        vec!["T-3"]
    );

    let used: BTreeSet<String> = ["T-3".to_string()].into_iter().collect();
    assert!(
        nearest_unused(&in_range, &used, 4).is_none(),
        "no in-range task left: the planner reports pool_exhausted, not band 3"
    );
}
