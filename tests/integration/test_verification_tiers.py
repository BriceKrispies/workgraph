from __future__ import annotations

import sys

import pytest

from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.core.models.enums import AttemptStatus
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.core.services import resolve
from workgraph.core.services.attempts import new_attempt
from workgraph.persistence.orm import Task


def setup_tiered_task(app, human, ws, make_repo):
    repo = make_repo("svc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(
        human, ws, TaskCreate(title="Tiered checks", repositories=["svc"], status="ready")
    )
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="svc",
        name="ci",
        checks=[
            {
                "name": "lint",
                "kind": "lint",
                "tier": "fast",
                "command": [sys.executable, "-c", "print('lint ok')"],
            },
            {"name": "unit tests", "command": [sys.executable, "-c", "print('1 passed')"]},
        ],
    )
    return task


def check_names(run) -> set[str]:
    return {c.name for c in run.checks}


def test_fast_tier_runs_only_fast_checks(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    run = app.verification.run(human, ws, task.key, tier="fast")
    assert check_names(run) == {"lint"}
    assert run.required_passed


def test_full_tier_runs_only_full_checks(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    run = app.verification.run(human, ws, task.key, tier="full")
    assert check_names(run) == {"unit tests"}


def test_no_tier_runs_every_check(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    run = app.verification.run(human, ws, task.key)
    assert check_names(run) == {"lint", "unit tests"}
    assert run.required_passed


def test_tier_without_matching_checks_is_rejected(app, human, ws, make_repo):
    repo = make_repo("only")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(human, ws, TaskCreate(title="Full only", repositories=["only"], status="ready"))
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="only",
        name="ci",
        checks=[{"name": "unit tests", "command": [sys.executable, "-c", "print('1 passed')"]}],
    )
    with pytest.raises(ValidationFailed, match="no fast verification checks"):
        app.verification.run(human, ws, task.key, tier="fast")


def test_fast_only_run_cannot_accept_task(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    with app.db.session(write=True) as session:
        session.get(Task, task.id).status = "awaiting_review"
    app.verification.run(human, ws, task.key, tier="fast")
    with pytest.raises(ConflictError, match="ran only fast checks"):
        app.verification.accept(human, ws, task.key)
    app.verification.run(human, ws, task.key)
    acceptance = app.verification.accept(human, ws, task.key)
    assert acceptance.mode == "automated"
    assert app.tasks.get(human, ws, task.key).status == "accepted"


def test_agent_manifest_lists_fast_checks_for_the_agent(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    with app.db.session(write=True) as session:
        attempt = new_attempt(
            session,
            human,
            resolve.task(session, None, task.id),
            provider="test",
            adapter="test",
            status=AttemptStatus.QUEUED,
        )
        attempt_id = attempt.id
    manifest = app.worktrees.prepare(human, attempt_id)
    assert {c["name"] for c in manifest.verification} == {"lint", "unit tests"}
    assert [c["name"] for c in manifest.agent_checks] == ["lint"]
    assert manifest.agent_checks[0]["command"] == [sys.executable, "-c", "print('lint ok')"]
    assert manifest.agent_checks[0]["repository"] == manifest.verification[0]["repository"]
