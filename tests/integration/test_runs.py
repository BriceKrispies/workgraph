from __future__ import annotations

import threading
import time

import pytest

from tests.helpers import drain, enable_fake_agent, failing_check, passing_check
from workgraph.core.errors import PermissionDenied, ProviderUnavailableError
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.jobs.worker import Worker


def make_task(app, human, ws, make_repo, names=("api", "sdk"), **kw):
    for name in names:
        repo = make_repo(name)
        app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    return app.tasks.create(
        human, ws, TaskCreate(title="Implement login", repositories=list(names), status="ready", **kw)
    )


def test_execution_is_opt_in(app, human, ws, make_repo):
    task = make_task(app, human, ws, make_repo, names=("solo",))
    with pytest.raises(PermissionDenied):
        app.runs.start(human, ws, task.key)


def test_unavailable_provider_is_actionable_and_harmless(app, human, ws, make_repo):
    enable_fake_agent(app)
    config = app.config
    from workgraph.config.settings import CommandAgentConfig

    config.agents["ghost"] = CommandAgentConfig(executable="definitely-not-installed-agent")
    app.config_store.save_global(config)
    task = make_task(app, human, ws, make_repo, names=("solo",))
    with pytest.raises(ProviderUnavailableError) as info:
        app.runs.start(human, ws, task.key, agent="ghost")
    assert "not found" in info.value.message
    assert app.tasks.get(human, ws, task.key).status == "ready"
    assert app.attempts.list(human, ws, task=task.key).total == 0


def test_successful_cross_repo_run_with_verification(app, human, ws, make_repo):
    enable_fake_agent(app)
    task = make_task(app, human, ws, make_repo)
    app.verification.create_config(
        human, ws, scope_type="task", scope=task.key, name="checks", checks=[passing_check()]
    )
    attempt = app.runs.start(human, ws, task.key)
    assert attempt.status == "queued"
    drain(app)
    run = app.runs.get(human, attempt.id)
    a = run.attempt
    assert a.status == "succeeded", a.error
    assert a.usage == {"input_tokens": 1200, "output_tokens": 340}
    assert a.usage_source == "command_reported"
    assert a.cost_usd is None  # never invented
    assert a.duration_ms is not None and a.queue_wait_ms is not None
    assert len(a.modified_repository_ids) == 2
    assert a.baseline_manifest_id and a.outcome_manifest_id
    assert a.telemetry["progress_events"] >= 2
    detail = app.tasks.get(human, ws, task.key)
    assert detail.status == "awaiting_review"
    commits = app.git.task_commits(human, ws, task.key)
    assert len(commits) == 2
    assert all(any(link["source"] == "execution_context" for link in c["links"]) for c in commits)
    runs = app.verification.list_runs(human, ws, task=task.key)
    assert runs[0].status == "passed" and len(runs[0].manifest) == 2
    acceptance = app.verification.accept(human, ws, task.key)
    assert acceptance.mode == "automated"
    assert app.tasks.get(human, ws, task.key).status == "accepted"
    sets = app.git.change_sets(human, ws, task.key)
    assert sets[0]["status"] == "verified"
    events = [e.event_type for e in app.history.task_history(human, ws, task.key)]
    assert "attempt.succeeded" in events and "task.accepted" in events


def test_failed_verification_then_retry(app, human, ws, make_repo):
    enable_fake_agent(app)
    task = make_task(app, human, ws, make_repo, names=("svc",))
    config = app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="svc",
        name="ci",
        checks=[failing_check(), {**passing_check("lint"), "required": False}],
    )
    first = app.runs.start(human, ws, task.key)
    drain(app)
    a1 = app.attempts.get(human, first.id)
    assert a1.status == "failed" and a1.exit_reason == "verification_failed"
    assert app.tasks.get(human, ws, task.key).status == "failed"
    failures = app.verification.failures(human, ws)
    assert failures[0]["failure_signatures"] == ["test:tests/test_api.py::test_login"]
    with pytest.raises(Exception, match="failed required checks"):
        app.verification.accept(human, ws, task.key)
    # operator fixes the check configuration and retries
    app.verification.create_config(
        human, ws, scope_type="repository", scope="svc", name="ci", checks=[passing_check()]
    )
    assert config.version == 1
    second = app.runs.retry(human, first.id)
    assert second.retry_of_attempt_id == first.id
    drain(app)
    assert app.attempts.get(human, second.id).status == "succeeded"
    assert app.tasks.get(human, ws, task.key).status == "awaiting_review"
    attempts = app.attempts.list(human, ws, task=task.key)
    assert attempts.total == 2


def test_agent_failure_crash_and_read_only_violation(app, human, ws, make_repo):
    enable_fake_agent(app, mode="fail")
    task = make_task(app, human, ws, make_repo, names=("one",))
    attempt = app.runs.start(human, ws, task.key)
    drain(app)
    out = app.attempts.get(human, attempt.id)
    assert out.status == "failed" and "simulated failure" in (out.error or "")
    enable_fake_agent(app, mode="crash")
    attempt2 = app.runs.start(human, ws, task.key)
    drain(app)
    crashed = app.attempts.get(human, attempt2.id)
    assert crashed.status == "failed" and crashed.exit_code == 3
    assert "boom" in (crashed.error or "")
    # read-only repository modified by the agent -> policy violation, attempt fails
    enable_fake_agent(app, mode="touch-ro")
    repo = make_repo("readonly")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path), access_mode="read_only"))
    app.tasks.link_repository(human, ws, task.key, "readonly")
    attempt3 = app.runs.start(human, ws, task.key)
    drain(app)
    violated = app.attempts.get(human, attempt3.id)
    assert violated.status == "failed" and "read-only" in (violated.error or "")


def test_timeout(app, human, ws, make_repo):
    enable_fake_agent(app, mode="sleep", timeout=2)
    task = make_task(app, human, ws, make_repo, names=("slow",))
    attempt = app.runs.start(human, ws, task.key)
    drain(app, timeout=60)
    out = app.attempts.get(human, attempt.id)
    assert out.status == "timed_out"
    assert app.tasks.get(human, ws, task.key).status == "failed"


def test_cancel_running_attempt(app, human, ws, make_repo):
    enable_fake_agent(app, mode="sleep")
    task = make_task(app, human, ws, make_repo, names=("cancel-me",))
    attempt = app.runs.start(human, ws, task.key)
    worker = Worker(app, concurrency=1, poll_interval=0.05, maintenance_interval=1)
    thread = threading.Thread(target=worker.run, kwargs={"max_idle_seconds": 20, "grace_seconds": 20})
    thread.start()
    for _ in range(200):
        if app.attempts.get(human, attempt.id).status == "running":
            break
        time.sleep(0.05)
    app.runs.cancel(human, attempt.id)
    for _ in range(400):
        if app.attempts.get(human, attempt.id).status == "cancelled":
            break
        time.sleep(0.05)
    worker.stop()
    thread.join(timeout=30)
    assert app.attempts.get(human, attempt.id).status == "cancelled"
    assert app.tasks.get(human, ws, task.key).status == "ready"


def test_cancel_queued_attempt(app, human, ws, make_repo):
    enable_fake_agent(app)
    task = make_task(app, human, ws, make_repo, names=("queued",))
    attempt = app.runs.start(human, ws, task.key)
    out = app.runs.cancel(human, attempt.id)
    assert out.status == "cancelled"
    assert app.tasks.get(human, ws, task.key).status == "ready"


def test_scheduler_runs_independent_tasks_and_respects_dependencies(app, human, ws, make_repo):
    enable_fake_agent(app)
    config = app.config
    config.execution.auto_schedule = True
    config.execution.max_concurrent_runs = 2
    app.config_store.save_global(config)
    for name in ("r1", "r2"):
        app.repositories.register(human, ws, RepositoryRegister(path=str(make_repo(name).path)))
    a = app.tasks.create(
        human, ws, TaskCreate(title="A", repositories=["r1"], status="ready", auto_execute=True)
    )
    b = app.tasks.create(
        human, ws, TaskCreate(title="B", repositories=["r2"], status="ready", auto_execute=True)
    )
    c = app.tasks.create(
        human,
        ws,
        TaskCreate(title="C", repositories=["r1"], status="ready", auto_execute=True, depends_on=[a.key]),
    )
    eligible = {e["key"] for e in app.scheduler.eligible(human)}
    assert eligible == {a.key, b.key}
    assert app.scheduler.explain(human, ws, c.key)["reasons"] == [
        "status is blocked",
        "dependencies unsatisfied",
    ]
    tick = app.scheduler.tick(human)
    assert {s["task"] for s in tick["started"]} == {a.key, b.key}
    drain(app)
    assert app.tasks.get(human, ws, a.key).status == "awaiting_review"
    assert app.tasks.get(human, ws, b.key).status == "awaiting_review"
    assert app.tasks.get(human, ws, c.key).status == "blocked"


def test_rerun_reproduces_inputs(app, human, ws, make_repo):
    enable_fake_agent(app)
    task = make_task(app, human, ws, make_repo, names=("rr",))
    first = app.runs.start(human, ws, task.key)
    drain(app)
    app.verification.reject(human, ws, task.key, reason="want another try")
    result = app.runs.rerun(human, first.id)
    assert result["reproduced_baseline"] is True
    drain(app)
    second = app.attempts.get(human, result["attempt"]["id"])
    assert second.rerun_of_attempt_id == first.id
    comparison = app.attempts.compare(human, first.id, second.id)
    assert comparison["same_task"] and comparison["fields"]["provider"]["same"]
    base_a = comparison["manifests"]["a"]["baseline"][0]["sha"]
    base_b = comparison["manifests"]["b"]["baseline"][0]["sha"]
    assert base_a == base_b


def test_escalation_after_attempt_limit(app, human, ws, make_repo):
    enable_fake_agent(app, mode="fail")
    config = app.config
    config.execution.max_attempts_per_task = 2
    app.config_store.save_global(config)
    task = make_task(app, human, ws, make_repo, names=("esc",))
    first = app.runs.start(human, ws, task.key)
    drain(app)
    app.runs.retry(human, first.id)
    drain(app)
    out = app.tasks.get(human, ws, task.key)
    assert "needs-review" in out.tags
    assert "task.escalated" in [e.event_type for e in app.history.task_history(human, ws, task.key)]
