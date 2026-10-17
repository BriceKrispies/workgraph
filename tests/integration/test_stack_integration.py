"""Fast-tier gating and the integration merge queue, on real repositories."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

import pytest
from sqlalchemy import select

from tests.helpers import FAKE_AGENT, drain, enable_fake_agent, failing_check, passing_check
from workgraph.config.settings import CommandAgentConfig
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.integrations.git.cli import Git
from workgraph.persistence.orm import Event, Job, TaskComment, VerificationRun


def _enable_agent(app, name: str, file: str) -> None:
    """A fake agent that writes its change to ``file``, so two children can edit different files."""
    config = app.config
    config.agents[name] = CommandAgentConfig(
        executable=sys.executable,
        args=[FAKE_AGENT, "--mode", "success", "--file", file, "{manifest}"],
        timeout_seconds=120,
        supports_usage=True,
    )
    app.config_store.save_global(config)


@pytest.fixture
def tree(app, human, ws, make_repo):
    """A repository with a parent task and two agents editing different files."""
    enable_fake_agent(app)
    _enable_agent(app, "fake-a", "a.py")
    _enable_agent(app, "fake-b", "b.py")
    repo = make_repo("svc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent work", repositories=["svc"]))
    return repo, parent


@pytest.fixture
def shared_tree(app, human, ws, make_repo):
    """A repository whose workgraph-demo.json makes every child write the same line of shared.py."""
    enable_fake_agent(app)
    repo = make_repo("svc")
    plan = {"file": "shared.py", "content": "VALUE = '{task}'\n"}
    repo.commit("add demo plan", {"workgraph-demo.json": json.dumps(plan) + "\n"})
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


def _run(app, human, ws, key: str, agent: str | None = None) -> str:
    attempt = app.runs.start(human, ws, key, agent=agent)
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


def _items(app, human, ws, parent) -> dict[str, Any]:
    return {item.task_key: item for item in app.stacks.get(human, ws, parent.key).items}


def _gate(app, human, ws, checks: list[dict[str, Any]]) -> None:
    app.verification.create_config(
        human, ws, scope_type="repository", scope="svc", name="gate", checks=checks
    )


def _integration_path(app, stack) -> Path:
    return app.paths.worktrees_dir / "stacks" / stack.id / "svc"


def _event_types(app, stack_id: str) -> list[str]:
    with app.db.session() as session:
        return list(session.scalars(select(Event.event_type).where(Event.entity_id == stack_id)))


def test_independent_children_pass_the_fast_gate_and_merge_in_order(app, human, ws, tree):
    repo, parent = tree
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, failing_check("full suite")],
    )
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    stack = app.stacks.create(human, ws, parent.key)
    branch = stack.repositories[0].integration_branch
    assert branch == f"wg/stack/{parent.key.lower()}"

    first_head = _entry(app, human, _run(app, human, ws, first.key, "fake-a"), "outcome")["sha"]
    second_head = _entry(app, human, _run(app, human, ws, second.key, "fake-b"), "outcome")["sha"]

    items = _items(app, human, ws, parent)
    assert items[first.key].state == "merged" and items[second.key].state == "merged"
    assert items[first.key].merge_order == 1 and items[second.key].merge_order == 2
    assert app.stacks.get(human, ws, parent.key).status == "integrating"
    # The full tier has not run, so the children wait for the integration tip.
    assert app.tasks.get(human, ws, first.key).status == "awaiting_verification"
    assert app.tasks.get(human, ws, second.key).status == "awaiting_verification"

    repo.git("merge-base", "--is-ancestor", first_head, branch)
    repo.git("merge-base", "--is-ancestor", second_head, branch)
    assert repo.git("log", "-1", "--format=%s", branch) == (
        f"Merge {second.key}: Second change into {branch}"
    )
    worktree = _integration_path(app, stack)
    assert Git(worktree).is_clean()
    assert Git(worktree).head() == repo.git("rev-parse", branch)

    types = _event_types(app, stack.id)
    assert types.count("stack.integration_merged") == 2
    assert "stack.status_changed" in types


def test_merge_jobs_hold_the_repository_resource_key(app, human, ws, tree):
    _, parent = tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    child = _child(app, human, ws, parent, "Only change")
    stack = app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, child.key, "fake-a")

    repository_id = stack.repositories[0].repository_id
    with app.db.session() as session:
        jobs = list(session.scalars(select(Job).where(Job.kind == "stack_integrate")))
    assert jobs
    for job in jobs:
        assert job.payload["repository_id"] == repository_id
        assert f"repo:{repository_id}" in job.resource_keys


def test_stacked_child_merges_after_its_sibling(app, human, ws, tree):
    repo, parent = tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change", depends_on=[first.key])
    stack = app.stacks.create(human, ws, parent.key)
    branch = stack.repositories[0].integration_branch
    assert app.tasks.get(human, ws, second.key).status == "blocked"

    first_head = _entry(app, human, _run(app, human, ws, first.key, "fake-a"), "outcome")["sha"]
    assert app.tasks.get(human, ws, second.key).status == "ready"
    second_attempt = _run(app, human, ws, second.key, "fake-b")
    second_head = _entry(app, human, second_attempt, "outcome")["sha"]

    assert _entry(app, human, second_attempt)["target_ref"] == first_head
    items = _items(app, human, ws, parent)
    assert items[first.key].merge_order == 1
    assert items[second.key].state == "verified" and items[second.key].merge_order == 2
    repo.git("merge-base", "--is-ancestor", second_head, branch)


def test_conflicting_pair_sends_the_second_child_back_with_the_integration_tip(app, human, ws, shared_tree):
    repo, parent = shared_tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    stack = app.stacks.create(human, ws, parent.key)
    branch = stack.repositories[0].integration_branch
    _run(app, human, ws, first.key)
    tip = repo.git("rev-parse", branch)

    _run(app, human, ws, second.key)

    item = _items(app, human, ws, parent)[second.key]
    assert item.state == "conflict"
    conflict = json.loads(item.detail)["conflicts"]["svc"]
    assert conflict["paths"] == ["shared.py"]
    assert conflict["integration_tip"] == tip
    assert "rebase" in conflict["instruction"]
    task = app.tasks.get(human, ws, second.key)
    assert task.status == "ready"
    with app.db.session() as session:
        bodies = list(session.scalars(select(TaskComment.body).where(TaskComment.task_id == task.id)))
    assert any("shared.py" in body and tip in body for body in bodies)
    # The merge was aborted: the integration worktree is clean and the branch did not move.
    assert Git(_integration_path(app, stack)).is_clean()
    assert repo.git("rev-parse", branch) == tip

    retry = _run(app, human, ws, second.key)
    base = _prepared_bases(app, retry)["svc"]
    assert base["source"] == "integration_tip"
    assert base["integration_tip"] == tip
    assert _entry(app, human, retry)["target_ref"] == tip
    items = _items(app, human, ws, parent)
    assert items[second.key].state == "verified"
    assert items[first.key].state == "verified"
    assert items[second.key].merge_order == 2
    assert repo.git("log", "-1", "--format=%s", branch) == (
        f"Merge {second.key}: Second change into {branch}"
    )


def test_failing_fast_check_fails_the_attempt_and_leaves_the_item_pending(app, human, ws, tree):
    _, parent = tree
    _gate(app, human, ws, [{**failing_check("lint"), "kind": "lint", "tier": "fast"}])
    child = _child(app, human, ws, parent, "Broken change")
    stack = app.stacks.create(human, ws, parent.key)

    attempt = app.runs.start(human, ws, child.key, agent="fake-a")
    drain(app)

    failed = app.attempts.get(human, attempt.id)
    assert failed.status == "failed"
    assert "verification failed" in failed.error
    assert app.tasks.get(human, ws, child.key).status == "failed"
    assert _items(app, human, ws, parent)[child.key].state == "pending"
    assert "stack.integration_merged" not in _event_types(app, stack.id)


def test_task_outside_a_stack_is_verified_with_every_tier(app, human, ws, tree):
    _repo, _parent = tree
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, failing_check("full")],
    )
    task = app.tasks.create(human, ws, TaskCreate(title="Standalone", repositories=["svc"], status="ready"))

    attempt = app.runs.start(human, ws, task.key, agent="fake-a")
    drain(app)

    assert app.attempts.get(human, attempt.id).status == "failed"
    assert app.tasks.get(human, ws, task.key).status == "failed"
    with app.db.session() as session:
        run = session.scalar(select(VerificationRun).where(VerificationRun.task_id == task.id))
    assert run is not None and run.status == "failed"
    assert {entry["name"] for entry in run.config_snapshot} == {"types", "full"}


def test_integrate_requests_for_an_unchanged_stack_collapse_into_one_job(app, human, ws, tree):
    _, parent = tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    first = _child(app, human, ws, parent, "First change")
    stack = app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, first.key, "fake-a")
    assert _items(app, human, ws, parent)[first.key].state == "verified"

    one = app.stacks.integrate(human, ws, parent.key)
    two = app.stacks.integrate(human, ws, parent.key)
    drain(app)

    assert one["jobs"] == two["jobs"]
    assert one["jobs"][0]["repository"] == "svc"
    assert _items(app, human, ws, parent)[first.key].state == "verified"
    assert app.stacks.get(human, ws, parent.key).id == stack.id


def test_stack_child_without_fast_checks_is_ready_without_a_verification_run(app, human, ws, tree):
    _, parent = tree
    _gate(app, human, ws, [failing_check("full suite")])
    child = _child(app, human, ws, parent, "Full-only change")
    app.stacks.create(human, ws, parent.key)

    attempt_id = _run(app, human, ws, child.key, "fake-a")

    assert app.tasks.get(human, ws, child.key).status == "awaiting_verification"
    assert _items(app, human, ws, parent)[child.key].state == "merged"
    with app.db.session() as session:
        assert session.scalar(select(VerificationRun.id).where(VerificationRun.task_id == child.id)) is None
    assert app.attempts.get(human, attempt_id).status == "succeeded"


def test_integration_worktree_survives_recovery_and_cleanup(app, human, ws, tree):
    _, parent = tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    child = _child(app, human, ws, parent, "Only change")
    stack = app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, child.key, "fake-a")
    path = str(_integration_path(app, stack))

    app.worktrees.recover(human)

    assert path in {row["path"] for row in app.worktrees.list(human, status="active")}
    assert path not in {row["path"] for row in app.worktrees.cleanup(human, dry_run=True)}
    assert Git(path).is_clean()


def _combined_check() -> dict[str, object]:
    """A full check that fails only when a.txt and c.txt both exist, so no single child breaks it."""
    code = (
        "import os\n"
        "if os.path.exists('a.txt') and os.path.exists('c.txt'):\n"
        "    print('FAILED combined::a_and_c - a.txt and c.txt both exist')\n"
        "    raise SystemExit(1)\n"
    )
    return {"name": "combined", "kind": "test", "command": [sys.executable, "-c", code]}


def _branch_files(repo, branch: str) -> set[str]:
    return set(repo.git("ls-tree", "--name-only", branch).split())


def _task_comments(app, task_id: str) -> list[str]:
    with app.db.session() as session:
        return list(session.scalars(select(TaskComment.body).where(TaskComment.task_id == task_id)))


def _verification_runs(app, parent) -> list[VerificationRun]:
    with app.db.session() as session:
        return list(session.scalars(select(VerificationRun).where(VerificationRun.task_id == parent.id)))


def test_bisect_ejects_the_child_that_completes_a_failing_combination(app, human, ws, tree):
    repo, parent = tree
    _enable_agent(app, "fake-a", "a.txt")
    _enable_agent(app, "fake-b", "b.txt")
    _enable_agent(app, "fake-c", "c.txt")
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, _combined_check()],
    )
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    third = _child(app, human, ws, parent, "Third change")
    stack = app.stacks.create(human, ws, parent.key)
    branch = stack.repositories[0].integration_branch

    _run(app, human, ws, first.key, "fake-a")
    _run(app, human, ws, second.key, "fake-b")
    assert _verification_runs(app, parent) == []
    _run(app, human, ws, third.key, "fake-c")

    items = _items(app, human, ws, parent)
    assert items[first.key].state == "verified"
    assert items[second.key].state == "verified"
    assert items[third.key].state == "ejected"
    ejection = json.loads(items[third.key].detail)["ejection"]
    assert ejection["position"] == 3
    assert ejection["failed_checks"] == ["combined"]
    assert ejection["signatures"] == {"combined": ["test:combined::a_and_c"]}
    failed_run = app.verification.get_run(human, ejection["verification_run_id"])
    assert failed_run.required_passed is False
    assert failed_run.task_id == parent.id

    third_task = app.tasks.get(human, ws, third.key)
    assert third_task.status == "ready"
    assert any("combined" in body for body in _task_comments(app, third_task.id))

    files = _branch_files(repo, branch)
    assert {"a.txt", "b.txt"} <= files
    assert "c.txt" not in files
    assert app.tasks.get(human, ws, first.key).status == "awaiting_review"
    assert app.tasks.get(human, ws, second.key).status == "awaiting_review"
    assert app.stacks.get(human, ws, parent.key).status == "verified"

    types = _event_types(app, stack.id)
    for event_type in (
        "stack.verification_started",
        "stack.verification_failed",
        "stack.bisect_probe",
        "stack.item_ejected",
        "stack.rebuilt",
        "stack.verified",
    ):
        assert event_type in types
    assert types.count("stack.bisect_probe") >= 2


def test_all_passing_full_checks_verify_every_item_with_one_run(app, human, ws, tree):
    _repo, parent = tree
    _enable_agent(app, "fake-a", "a.txt")
    _enable_agent(app, "fake-b", "b.txt")
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, passing_check("suite")],
    )
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    stack = app.stacks.create(human, ws, parent.key)

    _run(app, human, ws, first.key, "fake-a")
    assert _verification_runs(app, parent) == []
    _run(app, human, ws, second.key, "fake-b")

    items = _items(app, human, ws, parent)
    assert items[first.key].state == "verified" and items[second.key].state == "verified"
    assert app.tasks.get(human, ws, first.key).status == "awaiting_review"
    assert app.tasks.get(human, ws, second.key).status == "awaiting_review"
    assert app.stacks.get(human, ws, parent.key).status == "verified"
    assert [run.status for run in _verification_runs(app, parent)] == ["passed"]
    assert "stack.verified" in _event_types(app, stack.id)

    # An on-demand request verifies the tip again, as a new run.
    job_id = app.stack_verification.request(human, ws, parent.key)["job_id"]
    drain(app)
    assert app.stacks.get(human, ws, parent.key).status == "verified"
    assert len(_verification_runs(app, parent)) == 2
    with app.db.session() as session:
        assert session.get(Job, job_id).status == "succeeded"


def test_a_failure_on_the_base_ejects_nothing(app, human, ws, tree):
    _repo, parent = tree
    _enable_agent(app, "fake-a", "a.txt")
    _enable_agent(app, "fake-b", "b.txt")
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, failing_check("suite")],
    )
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    stack = app.stacks.create(human, ws, parent.key)

    _run(app, human, ws, first.key, "fake-a")
    _run(app, human, ws, second.key, "fake-b")

    items = _items(app, human, ws, parent)
    assert items[first.key].state == "merged" and items[second.key].state == "merged"
    assert app.stacks.get(human, ws, parent.key).status == "integrating"
    assert app.tasks.get(human, ws, first.key).status == "awaiting_verification"
    types = _event_types(app, stack.id)
    assert "stack.verification_failed_on_base" in types
    assert "stack.item_ejected" not in types
    assert "stack.verified" not in types


def test_a_dependent_of_the_culprit_returns_to_pending(app, human, ws, tree):
    repo, parent = tree
    _enable_agent(app, "fake-a", "a.txt")
    _enable_agent(app, "fake-b", "b.txt")
    _enable_agent(app, "fake-c", "c.txt")
    _gate(
        app,
        human,
        ws,
        [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}, _combined_check()],
    )
    first = _child(app, human, ws, parent, "First change")
    second = _child(app, human, ws, parent, "Second change")
    third = _child(app, human, ws, parent, "Third change", depends_on=[second.key])
    stack = app.stacks.create(human, ws, parent.key)
    branch = stack.repositories[0].integration_branch

    _run(app, human, ws, first.key, "fake-a")
    _run(app, human, ws, second.key, "fake-c")
    _run(app, human, ws, third.key, "fake-b")

    items = _items(app, human, ws, parent)
    assert items[second.key].state == "ejected"
    assert items[third.key].state == "pending"
    assert json.loads(items[third.key].detail)["reason"] == f"dependency {second.key} was ejected"
    assert items[first.key].state == "merged"
    assert app.tasks.get(human, ws, second.key).status == "ready"
    assert app.stacks.get(human, ws, parent.key).status == "integrating"
    files = _branch_files(repo, branch)
    assert "a.txt" in files
    assert "b.txt" not in files and "c.txt" not in files


def test_stack_verify_jobs_hold_the_repository_keys(app, human, ws, tree):
    _repo, parent = tree
    _gate(app, human, ws, [{**passing_check("types"), "kind": "typecheck", "tier": "fast"}])
    child = _child(app, human, ws, parent, "Only change")
    stack = app.stacks.create(human, ws, parent.key)
    _run(app, human, ws, child.key, "fake-a")

    repository_id = stack.repositories[0].repository_id
    with app.db.session() as session:
        jobs = list(session.scalars(select(Job).where(Job.kind == "stack_verify")))
    assert len(jobs) == 1
    job = jobs[0]
    assert job.payload == {"stack_id": stack.id}
    assert f"repo:{repository_id}" in job.resource_keys
    assert f"exclusive:stack-integration:{repository_id}" in job.resource_keys
    # Settling again on the same integration tips collapses into the existing job.
    assert app.stack_verification.enqueue_if_settled(human, stack.id) == job.id
