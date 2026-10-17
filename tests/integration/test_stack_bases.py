"""Attempt bases and dependency satisfaction for stack children, on real repositories."""

from __future__ import annotations

from typing import Any

import pytest
from sqlalchemy import select

from tests.helpers import drain, enable_fake_agent
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.persistence.orm import Event


@pytest.fixture
def tree(app, human, ws, make_repo):
    """A repository with a parent task; children are created per test."""
    enable_fake_agent(app)
    repo = make_repo("svc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent work", repositories=["svc"]))
    return repo, parent


def _child(app, human, ws, parent, title: str, *, depends_on: list[str] | None = None) -> Any:
    return app.tasks.create(
        human,
        ws,
        TaskCreate(
            title=title,
            parent=parent.key,
            repositories=["svc"],
            status="ready",
            depends_on=depends_on or [],
        ),
    )


def _run(app, human, ws, key: str) -> str:
    attempt = app.runs.start(human, ws, key)
    drain(app)
    assert app.attempts.get(human, attempt.id).status == "succeeded"
    return attempt.id


def _entry(app, human, attempt_id: str, kind: str = "baseline") -> dict[str, Any]:
    manifest = next(m for m in app.attempts.manifests(human, attempt_id) if m["kind"] == kind)
    return manifest["entries"][0]


def _prepared_bases(app, attempt_id: str) -> dict[str, Any]:
    with app.db.session() as session:
        return session.scalar(
            select(Event.data).where(
                Event.attempt_id == attempt_id, Event.event_type == "attempt.workspace_prepared"
            )
        )["bases"]


def test_independent_child_starts_at_stack_base_after_main_moves(app, human, ws, tree):
    repo, parent = tree
    first = _child(app, human, ws, parent, "First change")
    _child(app, human, ws, parent, "Second change")
    stack = app.stacks.create(human, ws, parent.key)
    base = repo.head()
    assert stack.repositories[0].base_sha == base
    repo.commit("main moves on", {"CHANGELOG.md": "later\n"})
    assert repo.head() != base

    attempt_id = _run(app, human, ws, first.key)

    entry = _entry(app, human, attempt_id)
    assert entry["target_ref"] == base and entry["sha"] == base
    assert entry["base"]["source"] == "stack_base"
    assert _prepared_bases(app, attempt_id)["svc"]["source"] == "stack_base"


def test_child_depending_on_sibling_starts_at_sibling_attempt_head(app, human, ws, tree):
    repo, parent = tree
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change", depends_on=[first.key])
    app.stacks.create(human, ws, parent.key)
    first_attempt = _run(app, human, ws, first.key)
    first_head = _entry(app, human, first_attempt, "outcome")["sha"]
    assert first_head != repo.head()
    assert app.tasks.get(human, ws, second.key).status == "blocked"

    app.stacks.set_item_state(human, ws, parent.key, first.key, "ready")
    assert app.tasks.get(human, ws, second.key).status == "ready"
    second_attempt = _run(app, human, ws, second.key)

    entry = _entry(app, human, second_attempt)
    assert entry["target_ref"] == first_head and entry["sha"] == first_head
    assert entry["base"] == {
        "source": "sibling_head",
        "stack_id": entry["base"]["stack_id"],
        "stack_base_sha": repo.head(),
        "sibling": first.key,
        "sibling_head": first_head,
        "superseded": [],
    }
    bases = _prepared_bases(app, second_attempt)
    assert bases["svc"]["sibling"] == first.key
    second_head = _entry(app, human, second_attempt, "outcome")["sha"]
    repo.git("merge-base", "--is-ancestor", first_head, second_head)


def test_non_linear_sibling_heads_fail_preparation_with_guidance(app, human, ws, tree):
    _, parent = tree
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    join = _child(app, human, ws, parent, "Join", depends_on=[first.key, second.key])
    app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, first.key)
    _run(app, human, ws, second.key)
    app.stacks.set_item_state(human, ws, parent.key, first.key, "ready")
    assert app.tasks.get(human, ws, join.key).status == "blocked"
    app.stacks.set_item_state(human, ws, parent.key, second.key, "ready")
    assert app.tasks.get(human, ws, join.key).status == "ready"

    attempt = app.runs.start(human, ws, join.key)
    drain(app)

    failed = app.attempts.get(human, attempt.id)
    assert failed.status == "failed"
    assert "not one line of history" in failed.error
    assert f"add an explicit dependency between {first.key} and {second.key}" in failed.error


def test_sibling_dependency_satisfied_at_ready_outside_dependency_needs_acceptance(app, human, ws, tree):
    _, parent = tree
    sibling = _child(app, human, ws, parent, "Stack sibling")
    dependent = _child(app, human, ws, parent, "Stack dependent", depends_on=[sibling.key])
    outside = app.tasks.create(
        human, ws, TaskCreate(title="Outside work", repositories=["svc"], status="ready")
    )
    outside_dependent = app.tasks.create(
        human,
        ws,
        TaskCreate(title="Needs outside", repositories=["svc"], status="ready", depends_on=[outside.key]),
    )
    app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, sibling.key)
    _run(app, human, ws, outside.key)
    assert app.tasks.get(human, ws, dependent.key).status == "blocked"
    assert app.tasks.get(human, ws, outside_dependent.key).status == "blocked"

    app.stacks.set_item_state(human, ws, parent.key, sibling.key, "ready")
    assert app.tasks.get(human, ws, dependent.key).status == "ready"
    assert app.tasks.get(human, ws, sibling.key).status == "awaiting_review"
    assert app.tasks.get(human, ws, outside_dependent.key).status == "blocked"

    app.verification.accept_manually(human, ws, outside.key, reason="reviewed")
    assert app.tasks.get(human, ws, outside_dependent.key).status == "ready"

    app.stacks.remove_item(human, ws, parent.key, sibling.key)
    assert app.tasks.get(human, ws, dependent.key).status == "blocked"


def test_task_outside_any_stack_keeps_default_base(app, human, ws, tree):
    repo, _ = tree
    task = app.tasks.create(human, ws, TaskCreate(title="Standalone", repositories=["svc"], status="ready"))
    repo.commit("main moves on", {"CHANGELOG.md": "later\n"})

    attempt_id = _run(app, human, ws, task.key)

    entry = _entry(app, human, attempt_id)
    assert entry["target_ref"] == "main"
    assert entry["sha"] == repo.head()
    assert entry["base"] is None
    assert "svc" not in _prepared_bases(app, attempt_id)
