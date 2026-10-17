from __future__ import annotations

import statistics

import pytest

from tests.helpers import drain, enable_fake_agent, failing_check, passing_check
from workgraph.analytics.estimation import percentile
from workgraph.analytics.service import AnalyticsFilters, AnalyticsQuery
from workgraph.config.settings import ModelSelection, OllamaProviderConfig
from workgraph.core.errors import ValidationFailed
from workgraph.core.models.enums import AttemptStatus
from workgraph.core.models.schemas import ClaimRequest, RelationCreate, RepositoryRegister, TaskCreate
from workgraph.core.services import resolve
from workgraph.core.services.attempts import finish_attempt, new_attempt
from workgraph.persistence.orm import AgentAttempt


def q(app, human, ws, metric, group_by=None, **filters):
    return app.analytics.query(human, ws, AnalyticsQuery(metric=metric, group_by=group_by or [],
                                                         filters=AnalyticsFilters(**filters)))


def add_attempt(app, human, task_id, *, provider, model, status, duration_ms, queue_ms):
    with app.db.session(write=True) as session:
        task = resolve.task(session, None, task_id)
        attempt = new_attempt(session, human, task, provider=provider, adapter="command", status=AttemptStatus.RUNNING,
                              model=model)
        finish_attempt(session, human, attempt, AttemptStatus(status), exit_reason="test", duration_ms=duration_ms)
        attempt.queue_wait_ms = queue_ms
        return attempt.id


def accept(app, human, ws, key):
    app.tasks.claim(human, ws, key, ClaimRequest())
    app.tasks.complete(human, ws, key)
    app.verification.accept_manually(human, ws, key, reason="ok")


def test_throughput_execution_and_agent_metrics(app, human, ws):
    durations = [1000, 2000, 3000, 4000, 10000]
    keys = []
    for i, duration in enumerate(durations):
        t = app.tasks.create(human, ws, TaskCreate(title=f"Task {i}", status="ready"))
        keys.append(t.key)
        add_attempt(app, human, t.id, provider="alpha" if i < 3 else "beta", model="m1", status="succeeded",
                    duration_ms=duration, queue_ms=100 * (i + 1))
    add_attempt(app, human, keys[0], provider="alpha", model="m1", status="failed", duration_ms=500, queue_ms=50)
    for key in keys[:3]:
        accept(app, human, ws, key)
    assert q(app, human, ws, "tasks_created")["rows"][0]["value"] == 5
    assert q(app, human, ws, "tasks_accepted")["rows"][0]["value"] == 3
    dist = q(app, human, ws, "execution_duration")
    row = dist["rows"][0]
    all_durations = [*durations, 500]
    assert row["n"] == 6
    assert row["median"] == pytest.approx(statistics.median(all_durations))
    assert row["p90"] == pytest.approx(percentile([float(d) for d in all_durations], 0.9))
    assert row["min"] == 500 and row["max"] == 10000
    agents = {(r["provider"], r["model"]): r for r in q(app, human, ws, "agent_completion_rate", ["provider", "model"])["rows"]}
    assert agents[("alpha", "m1")]["numerator"] == 3 and agents[("alpha", "m1")]["denominator"] == 4
    assert agents[("beta", "m1")]["value"] == 1.0
    fps = q(app, human, ws, "first_pass_success")["rows"][0]
    assert fps["denominator"] == 3 and fps["numerator"] == 2  # keys[0] needed two attempts
    by_period = q(app, human, ws, "tasks_created", ["period"])
    assert sum(r["value"] for r in by_period["rows"]) == 5
    result = q(app, human, ws, "cost")
    assert result["rows"][0]["n"] == 0  # no measured cost -> nothing invented
    assert "SELECT" in result["provenance"]["sql"] and result["metric"]["definition"]


def test_invalid_grouping_rejected(app, human, ws):
    with pytest.raises(ValidationFailed):
        q(app, human, ws, "tasks_created", ["model"])
    with pytest.raises(Exception):
        AnalyticsQuery(metric="DROP TABLE tasks")


def test_code_metrics_shared_commits_coupling_and_estimation(app, human, ws, make_repo):
    repos = {}
    for name in ("auth-service", "shared-sdk", "web-client"):
        repos[name] = make_repo(name)
        app.repositories.register(human, ws, RepositoryRegister(path=str(repos[name].path)))
    tasks = []
    for i in range(4):
        t = app.tasks.create(human, ws, TaskCreate(title=f"Token refresh part {i}", status="ready",
                                                   repositories=["auth-service", "shared-sdk"]))
        tasks.append(t)
        for repo in ("auth-service", "shared-sdk"):
            sha = repos[repo].commit(f"work {i}\n\nTask-ID: {t.key}", {f"src/mod{i}.py": "x = 1\n" * (i + 1)})
            app.git.sync_ref(human, ws, repo)
            assert sha
    shared_sha = repos["auth-service"].commit("shared fix", {"src/shared.py": "a\nb\nc\n"})
    app.git.sync_ref(human, ws, "auth-service")
    app.git.link_commit(human, ws, tasks[0].key, shared_sha)
    app.git.link_commit(human, ws, tasks[1].key, shared_sha)
    for t in tasks:
        accept(app, human, ws, t.key)
        app.git.compute_change_surface(human, t.id)
    volume = {r["repository_label"]: r for r in q(app, human, ws, "repository_change_volume", ["repository"])["rows"]}
    # shared commit counted once at repository level: 1+2+3+4 lines of work + 3 shared lines
    assert volume["auth-service"]["value"] == 10 + 3
    assert volume["auth-service"]["n"] == 5
    churn = q(app, human, ws, "cumulative_churn")["rows"][0]
    assert churn["n"] == 4
    by_repo = q(app, human, ws, "tasks_created", ["repository"])["rows"]
    assert {r["repository_label"]: r["value"] for r in by_repo} == {"auth-service": 4, "shared-sdk": 4}
    filtered = q(app, human, ws, "tasks_created", repository="web-client")["rows"]
    assert filtered[0]["value"] == 0
    coupling = app.analytics.coupling(human, ws)
    edge = coupling["edges"][0]
    assert {edge["source_name"], edge["target_name"]} == {"auth-service", "shared-sdk"}
    assert edge["co_changes"] == 4 and edge["relationship"] == "observed"
    assert len(edge["evidence"]) == 4 and edge["evidence"][0]["commits"]
    assert coupling["summary"]["cross_repository_tasks"] == 4
    assert coupling["coupling_candidates"]
    app.repositories.add_dependency(human, ws, "auth-service", "shared-sdk")
    assert app.analytics.coupling(human, ws)["edges"][0]["relationship"] == "declared+observed"
    new = app.tasks.create(human, ws, TaskCreate(title="Token refresh part 9", repositories=["auth-service"]))
    estimate = app.analytics.estimate(human, ws, task=new.key)
    assert estimate["status"] == "ok" and estimate["sample_size"] >= 3
    assert estimate["distributions"]["final_change_size"]["n"] >= 3
    assert all(b["reasons"] for b in estimate["basis"])
    lonely = app.analytics.estimate(human, ws, title="Completely unrelated quantum widget")
    assert lonely["status"] == "insufficient_data"
    history = app.analytics.task_history(human, ws, repository="shared-sdk")
    assert len(history) == 4


def test_failures_and_retrospective_fallback(app, human, ws, make_repo):
    enable_fake_agent(app)
    config = app.config
    config.llm.providers["ollama"] = OllamaProviderConfig(base_url="http://127.0.0.1:9", max_retries=0, timeout_seconds=1)
    config.llm.summarization = ModelSelection(provider="ollama", model="qwen")
    app.config_store.save_global(config)
    repo = make_repo("svc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(human, ws, TaskCreate(title="Flaky login", repositories=["svc"], status="ready"))
    app.verification.create_config(human, ws, scope_type="repository", scope="svc", name="ci", checks=[failing_check()])
    first = app.runs.start(human, ws, task.key)
    drain(app)
    second = app.runs.retry(human, first.id)
    drain(app)
    third = app.runs.retry(human, second.id)
    drain(app)
    report = app.analytics.failures(human, ws)
    kinds = {f["kind"] for f in report["findings"]}
    assert "frequently_failing_test" in kinds and "retry_loop" in kinds
    test_finding = next(f for f in report["findings"] if f["kind"] == "frequently_failing_test")
    assert task.key in test_finding["evidence"]["tasks"] and len(test_finding["evidence"]["attempts"]) == 3
    from workgraph.analytics.retrospective import generate_retrospective

    retro = generate_retrospective(app, human, ws, period_days=7)
    assert retro["narrative_source"] == "deterministic"
    assert "Retrospective" in retro["content"] and "Model narrative unavailable" in retro["content"]
    assert retro["facts"]["tasks"]["created"] == 1
    del third


def test_retrospective_with_model_narrative(app, human, ws):
    config = app.config
    config.llm.summarization = ModelSelection(provider="heuristic", model="heuristic-v1")
    app.config_store.save_global(config)
    t = app.tasks.create(human, ws, TaskCreate(title="Done thing", status="ready"))
    accept(app, human, ws, t.key)
    app.tasks.add_relation(human, ws, app.tasks.create(human, ws, TaskCreate(title="fix")).key,
                           RelationCreate(target=t.key, relation_type="regression_of"))
    from workgraph.analytics.retrospective import generate_retrospective

    retro = generate_retrospective(app, human, ws)
    assert retro["narrative_source"].startswith("model:heuristic")
    assert "model-generated interpretation" in retro["content"]
    assert q(app, human, ws, "regressions")["rows"][0]["value"] == 1


def test_exports(app, human, ws, tmp_path):
    app.tasks.create(human, ws, TaskCreate(title="exported"))
    for fmt in ("csv", "json", "parquet"):
        out = app.analytics.export(human, ws, target=tmp_path / f"facts.{fmt}", fmt=fmt, dataset="task_facts")
        assert out["bytes"] > 0
    out = app.analytics.export(human, ws, target=tmp_path / "q.csv", fmt="csv",
                               query=AnalyticsQuery(metric="tasks_created", group_by=["status"]))
    assert "backlog" in (tmp_path / "q.csv").read_text()
    with app.db.session() as session:
        assert session.query(AgentAttempt).count() == 0
