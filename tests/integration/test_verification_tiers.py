from __future__ import annotations

import sys

import pytest

from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.core.models.enums import AttemptStatus
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.core.services import resolve
from workgraph.core.services.attempts import new_attempt
from workgraph.persistence.orm import Task


@pytest.fixture
def manual_verification(app):
    """Tasks wait for `workgraph task accept` after a passing run: turns verification.auto_accept off."""
    config = app.config
    config.verification.auto_accept = False
    app.config_store.save_global(config)


def _await_verification(app, task) -> None:
    with app.db.session(write=True) as session:
        session.get(Task, task.id).status = "awaiting_verification"


def _acceptances(app, human, ws, task) -> list:
    return app.verification.acceptances(human, ws, task.key)


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


def test_failure_signature_comes_from_full_log_not_tail(app, human, ws):
    # The FAILED line is far above the last 200 lines the runner keeps as its tail.
    script = (
        "print('     Running unittests src/lib.rs (target/debug/deps/demo-0123456789abcdef)'); "
        "print('test a::b ... FAILED'); "
        "[print('noise') for _ in range(500)]; raise SystemExit(1)"
    )
    task = app.tasks.create(human, ws, TaskCreate(title="Cargo log", status="ready"))
    app.verification.create_config(
        human,
        ws,
        scope_type="workspace",
        scope=None,
        name="cargo",
        checks=[{"name": "cargo test", "parser": "cargo", "command": [sys.executable, "-c", script]}],
    )
    run = app.verification.run(human, ws, task.key)
    assert run.checks[0].status == "failed"
    assert run.checks[0].failure_signatures == ["test:demo::a::b"]


def test_passing_full_run_accepts_the_task(app, human, ws, make_repo):
    # Unstacked tasks are accepted only when the workspace does not land work through stacks.
    config = app.config
    config.stacks.auto_accept = False
    app.config_store.save_global(config)
    task = setup_tiered_task(app, human, ws, make_repo)
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key)
    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status == "accepted"
    (acceptance,) = _acceptances(app, human, ws, task)
    assert acceptance.mode == "automated"
    assert acceptance.reason == "verification.auto_accept"
    assert acceptance.verification_run_id == run.id


def test_unstacked_task_stays_awaiting_review_when_stacks_auto_accept(app, human, ws, make_repo):
    # With stacks.auto_accept on, work lands through stacks; an unstacked task must not bypass them.
    task = setup_tiered_task(app, human, ws, make_repo)
    assert app.config.verification.auto_accept and app.config.stacks.auto_accept
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key)
    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status == "awaiting_review"
    assert _acceptances(app, human, ws, task) == []


def test_auto_accept_off_leaves_the_task_awaiting_review(app, human, ws, make_repo, manual_verification):
    task = setup_tiered_task(app, human, ws, make_repo)
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key)
    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status == "awaiting_review"
    assert _acceptances(app, human, ws, task) == []


def test_fast_tier_run_does_not_auto_accept(app, human, ws, make_repo):
    task = setup_tiered_task(app, human, ws, make_repo)
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key, tier="fast")
    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status != "accepted"
    assert _acceptances(app, human, ws, task) == []


def test_failing_run_does_not_auto_accept(app, human, ws, make_repo):
    repo = make_repo("broken")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(human, ws, TaskCreate(title="Broken", repositories=["broken"], status="ready"))
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="broken",
        name="ci",
        checks=[{"name": "unit tests", "command": [sys.executable, "-c", "raise SystemExit(1)"]}],
    )
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key)
    assert not run.required_passed
    assert app.tasks.get(human, ws, task.key).status != "accepted"
    assert _acceptances(app, human, ws, task) == []


def test_run_without_required_checks_does_not_auto_accept(app, human, ws, make_repo):
    repo = make_repo("optional")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(
        human, ws, TaskCreate(title="Optional only", repositories=["optional"], status="ready")
    )
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="optional",
        name="ci",
        checks=[{"name": "unit tests", "required": False, "command": [sys.executable, "-c", "print('ok')"]}],
    )
    _await_verification(app, task)
    run = app.verification.run(human, ws, task.key)
    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status == "awaiting_review"
    assert _acceptances(app, human, ws, task) == []
