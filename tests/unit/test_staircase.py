"""Tests for the band staircase state machine."""

from __future__ import annotations

from typing import Any

import pytest

from workgraph.experiments.staircase import StaircaseConfig, next_step


def _config(**overrides: Any) -> StaircaseConfig:
    values: dict[str, Any] = {
        "min_band": 1,
        "max_band": 8,
        "per_visit": 2,
        "up": 1,
        "down": 2,
        "max_reversals": 3,
        "max_trials": 24,
        "refine_trials": 4,
        "refine_width": 1,
    }
    values.update(overrides)
    return StaircaseConfig(**values)


def test_starts_at_top_band() -> None:
    step = next_step(_config(), [])
    assert step.phase == "staircase"
    assert step.band == 8
    assert step.reversals == 0
    assert step.threshold is None
    assert step.visits == ()


def test_failing_visit_steps_down_by_down() -> None:
    step = next_step(_config(), [(8, False), (8, False)])
    assert step.band == 6
    assert step.visits == ((8, 0, 2),)
    assert step.threshold == 8.0


def test_one_pass_in_a_visit_counts_as_passed() -> None:
    step = next_step(_config(), [(4, True), (4, False)])
    assert step.band == 5


def test_passing_visit_steps_up_by_up() -> None:
    step = next_step(_config(up=2), [(3, True), (3, True)])
    assert step.band == 5


def test_clamps_at_the_top() -> None:
    step = next_step(_config(), [(8, True), (8, True)])
    assert step.band == 8


def test_clamps_at_the_bottom() -> None:
    step = next_step(_config(), [(2, False), (2, False)])
    assert step.band == 1
    step = next_step(_config(), [(1, False), (1, False)])
    assert step.band == 1


def test_incomplete_visit_stays_at_its_band() -> None:
    step = next_step(_config(), [(5, True)])
    assert step.band == 5
    assert step.phase == "staircase"
    assert step.visits == ()
    assert step.threshold is None


def test_incomplete_visit_after_complete_visits_continues_at_its_band() -> None:
    step = next_step(_config(), [(5, True), (5, False), (6, True)])
    assert step.band == 6
    assert step.visits == ((5, 1, 2),)


def test_visit_with_mixed_bands_raises() -> None:
    with pytest.raises(ValueError, match="mixes bands"):
        next_step(_config(), [(5, True), (6, False)])


def test_reversal_is_counted_including_clamped_moves() -> None:
    # Pass at the top is clamped but still an "up" move; the failure after it is a reversal.
    history = [(8, True), (8, True), (8, False), (8, False), (6, False), (6, False)]
    step = next_step(_config(), history)
    assert step.reversals == 1
    assert step.threshold == 8.0
    assert step.band == 4
    assert step.visits == ((8, 2, 2), (8, 0, 2), (6, 0, 2))


def test_no_reversal_when_direction_is_unchanged() -> None:
    history = [(8, False), (8, False), (6, False), (6, False)]
    step = next_step(_config(), history)
    assert step.reversals == 0
    assert step.threshold == 6.0


def test_threshold_is_mean_of_reversal_bands() -> None:
    # Down at 8, up at 6 (reversal), down at 7 (reversal): threshold is mean(6, 7).
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    step = next_step(_config(), history)
    assert step.reversals == 2
    assert step.threshold == 6.5
    assert step.phase == "staircase"


def test_threshold_without_reversal_is_last_visit_band() -> None:
    history = [(5, False), (5, False), (3, False), (3, False)]
    step = next_step(_config(), history)
    assert step.reversals == 0
    assert step.threshold == 3.0
    assert step.band == 1


def test_stops_at_max_reversals() -> None:
    # Down at 8, up at 6, down at 7: the third visit makes the second reversal.
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    step = next_step(_config(max_reversals=2, refine_trials=0), history)
    assert step.phase == "done"
    assert step.band is None
    assert step.reversals == 2


def test_stops_at_max_trials() -> None:
    history = [(5, True), (5, True), (6, False), (6, False)]
    step = next_step(_config(max_trials=4, refine_trials=0), history)
    assert step.phase == "done"
    assert step.threshold == 6.0


def test_max_trials_ends_the_phase_even_mid_visit() -> None:
    # The fifth trial opens a new visit at band 6; max_trials ends the phase there, and the open visit
    # is not counted in the visits summary.
    history = [(5, True), (5, True), (6, False), (6, False), (6, True)]
    step = next_step(_config(max_trials=5, refine_trials=0), history)
    assert step.phase == "done"
    assert step.visits == ((5, 2, 2), (6, 0, 2))


def test_reversal_stop_waits_for_visit_boundary() -> None:
    config = _config(max_reversals=1, refine_trials=0)
    at_boundary = [(8, False), (8, False), (6, True), (6, True)]
    assert next_step(config, at_boundary).phase == "done"
    # One more trial starts a visit: the staircase keeps running at that band.
    step = next_step(config, [*at_boundary, (7, False)])
    assert step.phase == "staircase"
    assert step.band == 7


def test_refine_starts_after_staircase_ends() -> None:
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    step = next_step(_config(max_reversals=2), history)
    assert step.phase == "refine"
    assert step.threshold == 6.5
    # round(6.5) == 6 and the window is [5, 7]; all counts are zero so the tie goes to the closest band,
    # which is 6 and 7 equally (0.5 away); the lower band wins.
    assert step.band == 6


def test_refine_prefers_band_with_fewest_trials() -> None:
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    config = _config(max_reversals=2)
    # Band 6 has one trial, so the untried bands 5 and 7 tie on count; 7 is closer to 6.5.
    assert next_step(config, history, [(6, True)]).band == 7
    # Bands 5, 6 and 7 are then all tried once and tie on count; 6 and 7 are equally close, so 6 wins.
    assert next_step(config, history, [(6, True), (7, False), (5, True)]).band == 6
    # Band 6 has the most trials, so the choice moves to the untried band 5.
    assert next_step(config, history, [(6, True), (7, False)]).band == 5


def test_refine_tie_on_count_and_distance_goes_to_lower_band() -> None:
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    config = _config(max_reversals=2)
    # Bands 6 and 7 are both at distance 0.5 from the threshold 6.5 and both have zero trials here.
    assert next_step(config, history, [(5, True)]).band == 6


def test_refine_is_done_after_its_trials() -> None:
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    refine = [(6, True), (7, False), (5, True), (6, False)]
    step = next_step(_config(max_reversals=2), history, refine)
    assert step.phase == "done"
    assert step.band is None


def test_refine_window_is_clamped_to_band_range() -> None:
    # Threshold 1.0 with width 2: window is [1, 3] because 1 is the bottom of the range.
    history = [(2, False), (2, False), (1, True), (1, True)]
    config = _config(max_reversals=1, min_band=1, refine_width=2)
    step = next_step(config, history)
    assert step.threshold == 1.0
    assert step.phase == "refine"
    assert step.band == 1
    step = next_step(config, history, [(1, True)])
    assert step.band == 2


def test_refine_with_zero_trials_is_done() -> None:
    history = [(8, False), (8, False), (6, True), (6, True), (7, False), (7, False)]
    step = next_step(_config(max_reversals=2, refine_trials=0), history)
    assert step.phase == "done"
    assert step.threshold == 6.5


def test_refine_before_staircase_finishes_raises() -> None:
    with pytest.raises(ValueError, match="refine"):
        next_step(_config(), [(8, False)], [(7, True)])


def test_from_protocol_reads_fields_and_defaults() -> None:
    protocol = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
        "refine": {"trials": 16, "width": 1},
    }
    config = StaircaseConfig.from_protocol(protocol)
    assert config == StaircaseConfig(
        min_band=1,
        max_band=9,
        per_visit=2,
        up=1,
        down=2,
        max_reversals=6,
        max_trials=24,
        refine_trials=16,
        refine_width=1,
    )
    no_refine = dict(protocol)
    del no_refine["refine"]
    defaulted = StaircaseConfig.from_protocol(no_refine)
    assert defaulted.refine_trials == 0
    assert defaulted.refine_width == 1


def test_from_protocol_names_the_missing_field() -> None:
    protocol = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 2, "up": 1, "max_reversals": 6, "max_trials": 24},
    }
    with pytest.raises(ValueError, match="down"):
        StaircaseConfig.from_protocol(protocol)


def test_from_protocol_names_the_invalid_field() -> None:
    protocol = {
        "bands": {"min": 1, "max": 9},
        "staircase": {"per_visit": 0, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
    }
    with pytest.raises(ValueError, match=r"staircase\.per_visit"):
        StaircaseConfig.from_protocol(protocol)


def test_config_rejects_min_above_max() -> None:
    with pytest.raises(ValueError, match="min_band"):
        _config(min_band=9, max_band=3)


def test_config_rejects_max_trials_below_one_visit() -> None:
    with pytest.raises(ValueError, match="max_trials"):
        _config(max_trials=1, per_visit=2)
