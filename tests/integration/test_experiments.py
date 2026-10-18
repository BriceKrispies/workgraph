from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest
from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, NotFoundError, PermissionDenied, ValidationFailed
from workgraph.core.models.schemas import TaskCreate, WorkspaceCreate
from workgraph.persistence.orm import Event
from workgraph.security.permissions import DEFAULT_AGENT_PERMISSIONS, Permission

PINNED = "claude-haiku-5-5"
OLDER = "claude-haiku-4-5-20251001"


def protocol(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "arms": [{"name": "haiku", "model": PINNED}],
        "brief_template": "leaf-v1",
        "primary_size": "source_lines",
        "bands": {"min": 1, "max": 5},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
        "budget": {"trials": 40},
    }
    base.update(overrides)
    return base


@pytest.fixture
def task(app, human, ws):
    return app.tasks.create(human, ws, TaskCreate(title="Measured change"))


@pytest.fixture
def running(app, human, ws):
    experiment = app.experiments.create(human, ws, "exp-one", "Experiment one", "Does it work?", protocol())
    return app.experiments.freeze(human, ws, experiment.slug)


def _events(app, entity_id: str) -> list[str]:
    with app.db.session() as session:
        return list(
            session.scalars(select(Event.event_type).where(Event.entity_id == entity_id).order_by(Event.seq))
        )


def test_create_then_freeze_then_close(app, human, ws):
    created = app.experiments.create(human, ws, "exp-one", "Experiment one", "Does it work?", protocol())
    assert (created.status, created.frozen_at, created.question) == ("draft", None, "Does it work?")
    frozen = app.experiments.freeze(human, ws, created.slug)
    assert frozen.status == "running" and frozen.frozen_at is not None
    closed = app.experiments.close(human, ws, created.id)
    assert closed.status == "closed"
    assert [e.slug for e in app.experiments.list(human, ws)] == ["exp-one"]
    assert [e.slug for e in app.experiments.list(human, ws, status="closed")] == ["exp-one"]
    assert app.experiments.list(human, ws, status="draft") == []
    assert app.experiments.get(human, ws, created.id).slug == "exp-one"
    assert _events(app, created.id) == ["experiment.created", "experiment.frozen", "experiment.closed"]


def test_slug_is_validated_and_unique_per_workspace(app, human, ws):
    with pytest.raises(ValidationFailed):
        app.experiments.create(human, ws, "Not A Slug", "Title", None, protocol())
    with pytest.raises(ValidationFailed):
        app.experiments.create(human, ws, "exp", "  ", None, protocol())
    app.experiments.create(human, ws, "exp", "Title", None, protocol())
    with pytest.raises(ConflictError):
        app.experiments.create(human, ws, "exp", "Again", None, protocol())


def test_frozen_protocol_cannot_change(app, human, ws):
    experiment = app.experiments.create(human, ws, "exp-one", "Experiment one", None, protocol())
    changed = protocol(budget={"trials": 12})
    assert app.experiments.update_protocol(human, ws, "exp-one", changed).protocol["budget"] == {"trials": 12}
    app.experiments.freeze(human, ws, "exp-one")
    with pytest.raises(ConflictError):
        app.experiments.update_protocol(human, ws, "exp-one", protocol(budget={"trials": 99}))
    with pytest.raises(ConflictError):
        app.experiments.freeze(human, ws, "exp-one")
    stored = app.experiments.get(human, ws, experiment.id)
    assert stored.protocol["budget"] == {"trials": 12}
    assert _events(app, experiment.id) == [
        "experiment.created",
        "experiment.protocol_updated",
        "experiment.frozen",
    ]


@pytest.mark.parametrize("alias", ["haiku", "sonnet", "opus", "fable", "inherit", "Haiku"])
def test_alias_model_ids_are_rejected(app, human, ws, alias):
    spec = protocol(arms=[{"name": "arm", "model": alias}])
    with pytest.raises(ValidationFailed, match=r"alias|pinned"):
        app.experiments.create(human, ws, "exp-alias", "Alias", None, spec)
    assert app.experiments.list(human, ws) == []


def test_pinned_full_model_ids_are_accepted(app, human, ws):
    spec = protocol(arms=[{"name": "haiku", "model": PINNED}, {"name": "older", "model": OLDER}])
    created = app.experiments.create(human, ws, "exp-pinned", "Pinned", None, spec)
    assert [arm["model"] for arm in created.protocol["arms"]] == [PINNED, OLDER]


def test_bad_protocol_reports_every_problem(app, human, ws):
    spec = protocol(
        arms=[{"name": "a", "model": "sonnet"}, {"name": "a", "model": PINNED}],
        bands={"min": 4, "max": 2},
        staircase={"per_visit": 0, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
        budget={"trials": "forty"},
        frobnicate=True,
    )
    del spec["primary_size"]
    with pytest.raises(ValidationFailed) as raised:
        app.experiments.create(human, ws, "exp-bad", "Bad", None, spec)
    problems = raised.value.details["problems"]
    joined = "\n".join(problems)
    assert "missing keys: primary_size" in joined
    assert "unknown keys: frobnicate" in joined
    assert "arms[0].model" in joined and "alias" in joined
    assert "arms[1].name 'a' is used by another arm" in joined
    assert "bands.min must not exceed bands.max" in joined
    assert "staircase.per_visit must be an integer >= 1" in joined
    assert "budget.trials must be an integer >= 1" in joined
    assert len(problems) == 7


def test_add_trial_requires_running_experiment_and_known_arm(app, human, ws, task):
    app.experiments.create(human, ws, "exp-one", "Experiment one", None, protocol())
    with pytest.raises(ConflictError):
        app.experiments.add_trial(human, ws, "exp-one", task.key, "haiku", "staircase")
    app.experiments.freeze(human, ws, "exp-one")
    with pytest.raises(ValidationFailed, match="not in the protocol"):
        app.experiments.add_trial(human, ws, "exp-one", task.key, "missing", "staircase")
    with pytest.raises(ValidationFailed, match="phase"):
        app.experiments.add_trial(human, ws, "exp-one", task.key, "haiku", "warmup")
    with pytest.raises(ValidationFailed, match="band 9"):
        app.experiments.add_trial(human, ws, "exp-one", task.key, "haiku", "staircase", band=9)
    first = app.experiments.add_trial(
        human, ws, "exp-one", task.key, "haiku", "staircase", band=3, features={"source_lines": 120}
    )
    second = app.experiments.add_trial(human, ws, "exp-one", task.key, "haiku", "repeat")
    assert (first.seq, second.seq) == (1, 2)
    assert (first.requested_model, first.status, first.task_key) == (PINNED, "planned", task.key)
    assert first.features == {"predicted": {"source_lines": 120}}
    app.experiments.close(human, ws, "exp-one")
    with pytest.raises(ConflictError):
        app.experiments.add_trial(human, ws, "exp-one", task.key, "haiku", "staircase")


def test_start_and_record_grade(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase", band=2)
    with pytest.raises(ConflictError):
        app.experiments.record_grade(human, ws, trial.id, "pass")
    started = app.experiments.start_trial(human, ws, trial.id, base_ref="abc123", worktree_path="/wt/1")
    assert (started.status, started.base_ref, started.worktree_path) == ("running", "abc123", "/wt/1")
    assert started.started_at is not None
    with pytest.raises(ConflictError):
        app.experiments.start_trial(human, ws, trial.id)
    graded = app.experiments.record_grade(
        human, ws, trial.id, "pass", outcome_detail={"gates": [0, 0]}, features={"source_lines": 80}
    )
    assert (graded.status, graded.outcome, graded.graded_at is not None) == ("graded", "pass", True)
    assert graded.outcome_detail == {"gates": [0, 0]}
    assert graded.features == {"predicted": {}, "actual": {"source_lines": 80}}
    with pytest.raises(ConflictError):
        app.experiments.record_grade(human, ws, trial.id, "fail")
    with pytest.raises(ValidationFailed, match="outcome"):
        app.experiments.record_grade(human, ws, trial.id, "maybe")
    assert app.experiments.trials(human, ws, running.slug)[0].status == "graded"
    assert app.experiments.trial(human, ws, trial.id).outcome == "pass"


def test_classify_validates_vocabularies_and_applies_to_failed_or_invalid(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    with pytest.raises(ConflictError):
        app.experiments.classify(human, ws, trial.id, "too_big", "agent")
    app.experiments.start_trial(human, ws, trial.id)
    app.experiments.record_grade(human, ws, trial.id, "pass")
    with pytest.raises(ConflictError):
        app.experiments.classify(human, ws, trial.id, "too_big", "agent")

    failed = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    app.experiments.start_trial(human, ws, failed.id)
    app.experiments.record_grade(human, ws, failed.id, "fail")
    with pytest.raises(ValidationFailed, match="failure cause"):
        app.experiments.classify(human, ws, failed.id, "nonsense", "agent")
    with pytest.raises(ValidationFailed, match="failure cause source"):
        app.experiments.classify(human, ws, failed.id, "too_big", "robot")
    classified = app.experiments.classify(human, ws, failed.id, "too_big", "agent", notes="whole feature")
    assert (classified.failure_cause, classified.failure_cause_source) == ("too_big", "agent")
    assert classified.notes == "whole feature"
    assert classified.status == "graded"

    excluded = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "live")
    marked = app.experiments.mark_invalid(human, ws, excluded.id, "harness", notes="runner crashed")
    assert (marked.status, marked.failure_cause, marked.failure_cause_source) == (
        "invalid",
        "harness",
        "human",
    )
    reclassified = app.experiments.classify(human, ws, excluded.id, "other", "human")
    assert reclassified.failure_cause == "other"


def test_link_run_with_matching_model_keeps_trial_valid(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    app.experiments.start_trial(human, ws, trial.id)
    linked = app.experiments.link_run(
        human, ws, trial.id, attempt_id="attempt-1", subagent_run_id="run-1", actual_model=PINNED
    )
    assert (linked.status, linked.actual_model, linked.attempt_id) == ("running", PINNED, "attempt-1")
    assert (linked.subagent_run_id, linked.failure_cause) == ("run-1", None)
    graded = app.experiments.record_grade(human, ws, trial.id, "pass")
    assert graded.status == "graded"


def test_link_run_with_other_model_marks_invalid_wrong_model(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    app.experiments.start_trial(human, ws, trial.id)
    linked = app.experiments.link_run(human, ws, trial.id, attempt_id="attempt-1", actual_model=OLDER)
    assert (linked.status, linked.actual_model, linked.requested_model) == ("invalid", OLDER, PINNED)
    assert (linked.failure_cause, linked.failure_cause_source) == ("wrong_model", "mechanical")
    graded = app.experiments.record_grade(human, ws, trial.id, "pass")
    assert (graded.status, graded.outcome) == ("invalid", "pass")
    assert _events(app, trial.id)[-3:] == [
        "experiment_trial.linked",
        "experiment_trial.invalidated",
        "experiment_trial.graded",
    ]


def test_link_from_subagent_run_markers(app, human, ws, task, running):
    matching = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    other = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "repeat")
    app.experiments.start_trial(human, ws, matching.id)
    app.experiments.start_trial(human, ws, other.id)

    run = SimpleNamespace(
        id="run-marked",
        attempt_id="attempt-9",
        telemetry={"trial_ids": [matching.id]},
        model=PINNED,
    )
    with app.db.session(write=True) as session:
        [linked] = app.experiments.link_run_markers(session, human, run)
    assert (linked.status, linked.subagent_run_id, linked.attempt_id) == (
        "running",
        "run-marked",
        "attempt-9",
    )

    drifted = SimpleNamespace(
        id="run-drift", attempt_id=None, telemetry={"trial_ids": [other.id]}, model=OLDER
    )
    with app.db.session(write=True) as session:
        [linked] = app.experiments.link_run_markers(session, human, drifted)
    assert (linked.status, linked.failure_cause) == ("invalid", "wrong_model")

    # A run without the columns, or without markers, names no trial and changes nothing.
    bare = SimpleNamespace(id="run-bare", attempt_id=None)
    empty = SimpleNamespace(id="run-empty", attempt_id=None, telemetry={"trial_ids": []}, model=PINNED)
    with app.db.session(write=True) as session:
        assert app.experiments.link_run_markers(session, human, bare) == []
        assert app.experiments.link_run_markers(session, human, empty) == []

    missing = SimpleNamespace(id="run-missing", attempt_id=None, telemetry={"trial_ids": ["not-a-trial"]})
    with app.db.session(write=True) as session, pytest.raises(NotFoundError):
        app.experiments.link_run_markers(session, human, missing)
    with pytest.raises(NotFoundError):
        app.experiments.link_from_subagent_run(human, "no-such-run")


def test_mark_invalid_and_classify_need_running_experiment(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    app.experiments.start_trial(human, ws, trial.id)
    app.experiments.record_grade(human, ws, trial.id, "fail")
    app.experiments.close(human, ws, running.slug)
    with pytest.raises(ConflictError):
        app.experiments.classify(human, ws, trial.id, "too_big", "agent")
    with pytest.raises(ConflictError):
        app.experiments.mark_invalid(human, ws, trial.id, "harness")


def test_every_mutation_records_an_event(app, human, ws, task):
    experiment = app.experiments.create(human, ws, "exp-events", "Events", None, protocol())
    app.experiments.update_protocol(human, ws, experiment.id, protocol(budget={"trials": 8}))
    app.experiments.freeze(human, ws, experiment.id)
    trial = app.experiments.add_trial(human, ws, experiment.id, task.key, "haiku", "staircase")
    app.experiments.start_trial(human, ws, trial.id)
    app.experiments.link_run(human, ws, trial.id, attempt_id="attempt-1", actual_model=PINNED)
    app.experiments.record_grade(human, ws, trial.id, "fail")
    app.experiments.classify(human, ws, trial.id, "missing_context", "human")
    app.experiments.close(human, ws, experiment.id)
    assert _events(app, experiment.id) == [
        "experiment.created",
        "experiment.protocol_updated",
        "experiment.frozen",
        "experiment.closed",
    ]
    assert _events(app, trial.id) == [
        "experiment_trial.added",
        "experiment_trial.started",
        "experiment_trial.linked",
        "experiment_trial.graded",
        "experiment_trial.classified",
    ]


def test_agents_can_read_but_not_mutate(app, human, ws, task, running):
    agent = RequestContext.agent("worker")
    assert Permission.EXPERIMENT_WRITE not in DEFAULT_AGENT_PERMISSIONS
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    with pytest.raises(PermissionDenied):
        app.experiments.create(agent, ws, "exp-agent", "Agent", None, protocol())
    with pytest.raises(PermissionDenied):
        app.experiments.freeze(agent, ws, running.slug)
    with pytest.raises(PermissionDenied):
        app.experiments.classify(agent, ws, trial.id, "too_big", "agent")
    assert app.experiments.get(agent, ws, running.slug).status == "running"
    assert [t.id for t in app.experiments.trials(agent, ws, running.slug)] == [trial.id]
    assert app.experiments.trial(agent, ws, trial.id).id == trial.id


def test_trial_references_are_scoped_to_the_workspace(app, human, ws, task, running):
    trial = app.experiments.add_trial(human, ws, running.slug, task.key, "haiku", "staircase")
    other = app.workspaces.create(human, WorkspaceCreate(slug="other", name="Other", key_prefix="OTH"))
    with pytest.raises(NotFoundError):
        app.experiments.trial(human, other.slug, trial.id)
    with pytest.raises(NotFoundError):
        app.experiments.get(human, other.slug, running.slug)
