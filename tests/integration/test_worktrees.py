from __future__ import annotations

from pathlib import Path

import pytest

from workgraph.core.errors import ResourceBusyError, ValidationFailed
from workgraph.core.models.enums import AttemptStatus
from workgraph.core.models.schemas import ClaimRequest, RepositoryRegister, TaskCreate
from workgraph.core.services import resolve
from workgraph.core.services.attempts import new_attempt
from workgraph.integrations.git.cli import Git


def setup_task(app, human, ws, make_repo, names=("api", "lib"), read_only=()):
    repos = {}
    for name in names:
        repo = make_repo(name)
        app.repositories.register(
            human,
            ws,
            RepositoryRegister(
                path=str(repo.path), access_mode="read_only" if name in read_only else "read_write"
            ),
        )
        repos[name] = repo
    task = app.tasks.create(
        human, ws, TaskCreate(title="Cross repo", repositories=list(names), status="ready")
    )
    with app.db.session(write=True) as session:
        t = resolve.task(session, None, task.id)
        attempt = new_attempt(session, human, t, provider="test", adapter="test", status=AttemptStatus.QUEUED)
        attempt_id = attempt.id
    return repos, task, attempt_id


def test_prepare_worktrees_manifest_and_change_set(app, human, ws, make_repo):
    repos, task, attempt_id = setup_task(
        app, human, ws, make_repo, names=("api", "lib", "docs"), read_only=("docs",)
    )
    manifest = app.worktrees.prepare(human, attempt_id)
    assert len(manifest.repositories) == 3
    by_name = {e.repository: e for e in manifest.repositories}
    assert by_name["api"].isolation == "worktree" and by_name["api"].branch.startswith("wg/wg-1/")
    assert by_name["docs"].isolation == "snapshot" and by_name["docs"].access == "read_only"
    for entry in manifest.repositories:
        assert Path(entry.checkout_path).exists()
        assert entry.sha == repos[entry.repository].head()
    assert manifest.change_set_id is not None
    # user checkouts are untouched
    assert Git(repos["api"].path).current_branch() == "main"
    # agent commits only in "api" -> change set partially applied
    api_wt = Path(by_name["api"].checkout_path)
    (api_wt / "feature.txt").write_text("x\n")
    Git(api_wt).run("add", "-A")
    Git(api_wt).run("-c", "user.name=a", "-c", "user.email=a@b", "commit", "-qm", "agent work")
    (Path(by_name["docs"].checkout_path) / "scribble.txt").write_text("nope\n")
    outcome = app.worktrees.capture_outcome(human, attempt_id)
    assert len(outcome["commits"][by_name["api"].repository_id]) == 1
    assert outcome["violations"] and outcome["violations"][0]["repository"] == "docs"
    sets = app.git.change_sets(human, ws, task.key)
    assert sets[0]["status"] == "partially_applied"
    statuses = {i["repository_id"]: i["status"] for i in sets[0]["items"]}
    assert statuses[by_name["api"].repository_id] == "committed"
    assert statuses[by_name["lib"].repository_id] == "pending"
    # execution-context attribution of the agent's commit
    assert app.git.link_attempt_commits(human, attempt_id) == 1
    # release keeps dirty worktrees, removes clean ones; branches are kept
    results = app.worktrees.release(human, attempt_id, remove=True)
    removed = {r["worktree_id"]: r["removed"] for r in results}
    assert removed[by_name["api"].worktree_id] is True
    assert removed[by_name["docs"].worktree_id] is False
    assert not api_wt.exists()
    assert "wg/wg-1/" in repos["api"].git("branch", "--list", "wg/*")


def test_in_place_isolation_uses_exclusive_lease(app, human, ws, make_repo):
    _repos, _task, attempt_a = setup_task(app, human, ws, make_repo, names=("solo",))
    task_b = app.tasks.create(human, ws, TaskCreate(title="Second", repositories=["solo"], status="ready"))
    with app.db.session(write=True) as session:
        t = resolve.task(session, None, task_b.id)
        attempt_b = new_attempt(
            session, human, t, provider="test", adapter="test", status=AttemptStatus.QUEUED
        ).id
    app.worktrees.prepare(human, attempt_a, isolation="in_place")
    with pytest.raises(ResourceBusyError):
        app.worktrees.prepare(human, attempt_b, isolation="in_place")
    app.worktrees.release(human, attempt_a)
    manifest = app.worktrees.prepare(human, attempt_b, isolation="in_place")
    assert manifest.repositories[0].isolation == "in_place"


def test_parallel_worktree_attempts_do_not_conflict(app, human, ws, make_repo):
    _repos, _task, attempt_a = setup_task(app, human, ws, make_repo, names=("shared",))
    task_b = app.tasks.create(human, ws, TaskCreate(title="B", repositories=["shared"], status="ready"))
    with app.db.session(write=True) as session:
        t = resolve.task(session, None, task_b.id)
        attempt_b = new_attempt(
            session, human, t, provider="test", adapter="test", status=AttemptStatus.QUEUED
        ).id
    m1 = app.worktrees.prepare(human, attempt_a)
    m2 = app.worktrees.prepare(human, attempt_b)
    assert m1.repositories[0].checkout_path != m2.repositories[0].checkout_path


def test_crash_recovery_of_worktrees(app, human, ws, make_repo):
    _repos, _task, attempt_id = setup_task(app, human, ws, make_repo, names=("crashy",))
    manifest = app.worktrees.prepare(human, attempt_id)
    path = Path(manifest.repositories[0].checkout_path)
    # simulate the attempt dying and the directory vanishing
    import shutil

    shutil.rmtree(path)
    report = app.worktrees.recover(human)
    assert str(path) in report["orphaned"]
    rows = app.worktrees.list(human, ws)
    assert rows[0]["status"] == "orphaned"


def test_attempt_start_claims_ready_task_and_prepares_worktree(app, human, ws, make_repo):
    repo = make_repo("api")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(
        human, ws, TaskCreate(title="External work", repositories=["api"], status="ready")
    )

    out = app.runs.start_external(human, ws, task.key, agent_name="tester", model="test-model")

    assert out.task_key == task.key and out.attempt_number == 1
    [entry] = out.repositories
    assert entry.repository == "api" and entry.base_sha == repo.head()
    assert entry.branch.startswith(f"wg/{task.key.lower()}/attempt-1-")
    assert entry.base is None
    checkout = Path(entry.checkout_path)
    assert checkout.exists() and Git(checkout).current_branch() == entry.branch
    assert Git(checkout).head() == repo.head()
    assert Git(repo.path).current_branch() == "main"
    started = app.tasks.get(human, ws, task.key)
    assert started.status == "running" and started.claimed_by == human.actor
    attempt = app.attempts.get(human, out.attempt_id)
    assert attempt.status == "running" and attempt.provider == "tester"
    assert attempt.adapter == "claude_code_subagent" and attempt.model == "test-model"
    assert attempt.baseline_manifest_id is not None
    assert app.worktrees.attempt_is_active(out.attempt_id)


def test_attempt_start_continues_a_claim_the_actor_holds(app, human, ws, make_repo):
    repo = make_repo("api")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(
        human, ws, TaskCreate(title="Claimed first", repositories=["api"], status="ready")
    )
    app.tasks.claim(human, ws, task.key, ClaimRequest())

    out = app.runs.start_external(human, ws, task.key)

    assert out.attempt_number == 1
    assert app.tasks.get(human, ws, task.key).status == "running"


def test_attempt_start_refuses_backlog_task_and_bad_isolation(app, human, ws, make_repo):
    repo = make_repo("api")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    backlog = app.tasks.create(human, ws, TaskCreate(title="Not yet", repositories=["api"]))
    with pytest.raises(ValidationFailed, match="backlog"):
        app.runs.start_external(human, ws, backlog.key)
    assert app.tasks.get(human, ws, backlog.key).status == "backlog"
    assert app.attempts.list(human, ws, task=backlog.key).total == 0

    ready = app.tasks.create(human, ws, TaskCreate(title="Ready", repositories=["api"], status="ready"))
    with pytest.raises(ValidationFailed, match="isolation"):
        app.runs.start_external(human, ws, ready.key, isolation="snapshot")
    assert app.tasks.get(human, ws, ready.key).status == "ready"
    assert app.attempts.list(human, ws, task=ready.key).total == 0
