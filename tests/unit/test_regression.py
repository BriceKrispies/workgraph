"""Judging a verification run against the base, for checks in regression mode."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

import pytest

from workgraph.integrations.git.regression import CheckOutcome, regressions
from workgraph.verification.configs import CheckSpec
from workgraph.verification.service import CheckResultOut, VerificationRunOut

NOW = datetime(2026, 1, 1, tzinfo=UTC)


def _check(name: str, status: str = "failed", signatures: list[str] | None = None, required: bool = True):
    return CheckResultOut(
        id=f"id-{name}",
        name=name,
        kind="test",
        required=required,
        repository_id=None,
        status=status,
        exit_code=None,
        duration_ms=None,
        timed_out=False,
        log_artifact_id=None,
        output_excerpt="",
        failure_signatures=signatures or [],
    )


def _run(*checks: CheckResultOut) -> VerificationRunOut:
    return VerificationRunOut(
        id="run-tip",
        workspace_id="ws",
        task_id="task",
        attempt_id=None,
        manifest_id=None,
        status="failed",
        required_passed=all(c.status == "passed" or not c.required for c in checks),
        triggered_by="test",
        started_at=NOW,
        finished_at=NOW,
        created_at=NOW,
        checks=list(checks),
    )


def _spec(name: str, baseline: str = "regression") -> CheckSpec:
    return CheckSpec(name=name, command=["check"], baseline=baseline)


def _specs(*specs: CheckSpec) -> dict[str, CheckSpec]:
    return {spec.name: spec for spec in specs}


def test_baseline_defaults_to_absolute_and_rejects_unknown_modes():
    assert CheckSpec(name="suite", command=["check"]).baseline == "absolute"
    with pytest.raises(ValueError):
        CheckSpec(name="suite", command=["check"], baseline="sometimes")  # type: ignore[arg-type]


def test_a_passed_check_is_never_failing():
    run = _run(_check("suite", status="passed"))
    assert regressions(run, {}, _specs(_spec("suite"))) == {}


def test_an_optional_failing_check_is_not_failing():
    run = _run(_check("lint", signatures=["test:x"], required=False))
    assert regressions(run, None, _specs(_spec("lint", "absolute"))) == {}


def test_an_absolute_failure_is_failing_whatever_the_base_says():
    run = _run(_check("suite", signatures=["test:a", "test:b"]))
    base = {"suite": CheckOutcome("failed", ["test:a", "test:b"])}
    assert regressions(run, base, _specs(_spec("suite", "absolute"))) == {"suite": ["test:a", "test:b"]}


def test_a_regression_failure_already_on_the_base_is_tolerated():
    run = _run(_check("suite", signatures=["test:a"]))
    base = {"suite": CheckOutcome("failed", ["test:a"])}
    assert regressions(run, base, _specs(_spec("suite"))) == {}


def test_a_known_failure_whose_test_moved_to_another_binary_is_tolerated():
    # spec02_055.rs merged into the spec02 binary: the test gains a module path, the failure is the same.
    run = _run(_check("suite", signatures=["test:spec02::spec02_055::a_device_run", "test:fresh"]))
    base = {"suite": CheckOutcome("failed", ["test:a_device_run", "test:spec02_055::a_device_run"])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": ["test:fresh"]}


def test_only_the_new_signatures_of_a_regression_failure_are_reported_in_order():
    run = _run(_check("suite", signatures=["test:c", "test:a", "test:b"]))
    base = {"suite": CheckOutcome("failed", ["test:a"])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": ["test:c", "test:b"]}


def test_a_regression_failure_is_failing_when_the_base_has_no_outcome():
    run = _run(_check("suite", signatures=["test:a"]))
    assert regressions(run, {}, _specs(_spec("suite"))) == {"suite": ["test:a"]}
    assert regressions(run, None, _specs(_spec("suite"))) == {"suite": ["test:a"]}


def test_a_regression_failure_is_failing_when_the_base_passed():
    run = _run(_check("suite", signatures=["test:a"]))
    base = {"suite": CheckOutcome("passed", [])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": ["test:a"]}


def test_a_timeout_is_failing_even_when_the_base_fails_the_same_way():
    run = _run(_check("suite", signatures=["timeout", "test:a"]))
    base = {"suite": CheckOutcome("failed", ["timeout", "test:a"])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": ["timeout", "test:a"]}


def test_an_unparseable_failure_is_failing_even_when_the_base_fails():
    run = _run(_check("suite", signatures=[]))
    base = {"suite": CheckOutcome("failed", [])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": []}


def test_checks_are_judged_by_their_own_mode():
    run = _run(
        _check("unit", signatures=["test:a"]),
        _check("integration", signatures=["test:b"]),
        _check("types", signatures=["error:x"]),
    )
    base = {"unit": CheckOutcome("failed", ["test:a"]), "integration": CheckOutcome("failed", ["test:b"])}
    specs = _specs(_spec("unit"), _spec("integration", "absolute"), _spec("types", "absolute"))
    assert regressions(run, base, specs) == {"integration": ["test:b"], "types": ["error:x"]}


def test_a_check_missing_from_the_specs_is_judged_absolutely():
    run = _run(_check("suite", signatures=["test:a"]))
    base = {"suite": CheckOutcome("failed", ["test:a"])}
    assert regressions(run, base, {}) == {"suite": ["test:a"]}


def test_an_errored_check_is_failing_in_absolute_mode_whatever_the_base_says():
    run = _run(_check("suite", status="error"))
    base = {"suite": CheckOutcome("error", [], "older")}
    assert regressions(run, base, _specs(_spec("suite", "absolute"))) == {"suite": []}


def test_an_errored_check_is_never_tolerated_in_regression_mode():
    run = _run(_check("suite", status="error", signatures=["test:a"]))
    base = {"suite": CheckOutcome("failed", ["test:a"])}
    assert regressions(run, base, _specs(_spec("suite"))) == {"suite": ["test:a"]}
    assert regressions(run, None, _specs(_spec("suite"))) == {"suite": ["test:a"]}


def test_an_errored_optional_check_is_not_failing():
    run = _run(_check("lint", status="error", required=False))
    assert regressions(run, None, _specs(_spec("lint", "absolute"))) == {}


def test_outcome_run_id_does_not_affect_the_judgement():
    run = _run(_check("suite", signatures=["test:a"]))
    base: dict[str, Any] = {"suite": CheckOutcome("failed", ["test:a"], run_id="older")}
    assert regressions(run, base, _specs(_spec("suite"))) == {}
