from __future__ import annotations

import socket

import pytest

from tests.fixtures.fake_ollama import FakeOllama
from tests.helpers import drain
from workgraph.config.settings import ModelSelection, OllamaProviderConfig
from workgraph.core.errors import ProviderUnavailableError
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate, TaskUpdate
from workgraph.integrations.llm.base import InvalidModelOutputError
from workgraph.integrations.llm.ollama import OllamaProvider


@pytest.fixture
def ollama():
    with FakeOllama() as server:
        yield server


def use_ollama(app, url, model="qwen-test", embed="embed-test"):
    config = app.config
    config.llm.providers["ollama"] = OllamaProviderConfig(base_url=url, timeout_seconds=5, max_retries=2, backoff_seconds=0)
    config.llm.classification = ModelSelection(provider="ollama", model=model)
    config.llm.embeddings = ModelSelection(provider="ollama", model=embed)
    app.config_store.save_global(config)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def test_ollama_structured_output_retries_and_errors(ollama):
    provider = OllamaProvider("ollama", OllamaProviderConfig(base_url=ollama.url, max_retries=2, backoff_seconds=0))
    assert provider.check_model("qwen-test")[0]
    assert not provider.check_model("missing")[0]
    ollama.fail_next = 2
    response = provider.generate_json("qwen-test", system="s", prompt="<task>Fix crash in login</task>",
                                      schema={"title": "TaskClassification"})
    assert response.data["category"] == "bug_fix" and response.attempts == 3
    sent = ollama.requests[-1]["body"]
    assert sent["format"] == {"title": "TaskClassification"} and sent["stream"] is False
    with pytest.raises(InvalidModelOutputError):
        provider.generate_json("broken-model", system="s", prompt="<task>x</task>", schema={})
    ollama.fail_next = 10
    with pytest.raises(ProviderUnavailableError):
        provider.generate_json("qwen-test", system="s", prompt="<task>x</task>", schema={})
    ollama.fail_next = 0
    with pytest.raises(ProviderUnavailableError):
        provider.generate_json("missing-model", system="s", prompt="<task>x</task>", schema={})
    assert len(provider.embed("embed-test", ["a", "b"])) == 2


def test_provider_unreachable():
    provider = OllamaProvider("ollama", OllamaProviderConfig(base_url=f"http://127.0.0.1:{free_port()}",
                                                            max_retries=1, backoff_seconds=0, timeout_seconds=2))
    ok, detail = provider.check_model("x")
    assert not ok and "unreachable" in detail
    with pytest.raises(ProviderUnavailableError):
        provider.generate_json("x", system="", prompt="", schema={})


def test_classification_job_versioning_and_model_policy(app, human, ws, ollama):
    use_ollama(app, ollama.url)
    config = app.config
    config.llm.auto_classify = True
    app.config_store.save_global(config)
    task = app.tasks.create(human, ws, TaskCreate(title="Fix crash when login token expires",
                                                  description="The app throws an exception and crashes."))
    drain(app)
    result = app.classification.get(human, ws, task.key)
    current = result.current
    assert current is not None and current.category == "bug_fix"
    assert current.provider == "ollama" and current.model == "qwen-test"
    assert current.prompt_version == "classify-v1" and current.taxonomy_version == "2026.1"
    assert current.input_hash and current.validation_status == "valid" and current.duration_ms is not None
    assert app.tasks.get(human, ws, task.key).category == "bug_fix"
    # A different model never silently replaces the current classification.
    ollama.forced_category = "refactor"
    other = app.classification.run_classification(human, task.id, model="other-model")
    assert other.category == "refactor" and not other.is_current
    assert app.classification.get(human, ws, task.key).current.id == current.id
    assert app.classification.get(human, ws, task.key).disagreement
    assert [d["task_key"] for d in app.classification.disagreements(human, ws)] == [task.key]
    # Same model + same input is idempotent.
    again = app.classification.run_classification(human, task.id)
    assert again.id == current.id
    # Material description change -> re-classification by the same model replaces it.
    app.tasks.update(human, ws, task.key, TaskUpdate(description="Refactor the login module into smaller units."))
    drain(app)
    updated = app.classification.get(human, ws, task.key).current
    assert updated.id != current.id and updated.model == "qwen-test"
    # Manual confirmation is authoritative.
    manual = app.classification.confirm(human, ws, task.key, category="security", note="actually a token issue")
    assert manual.is_current and manual.source == "manual"
    app.classification.run_classification(human, task.id, model="other-model", promote=False)
    assert app.classification.get(human, ws, task.key).current.id == manual.id
    # History is preserved, nothing destroyed.
    assert len(app.classification.get(human, ws, task.key).history) >= 4


def test_invalid_output_and_unavailable_model(app, human, ws, ollama):
    use_ollama(app, ollama.url, model="broken-model")
    task = app.tasks.create(human, ws, TaskCreate(title="Write docs"))
    out = app.classification.run_classification(human, task.id)
    assert out.status == "failed" and out.validation_status == "invalid_output"
    use_ollama(app, ollama.url, model="not-pulled")
    with pytest.raises(ProviderUnavailableError):
        app.classification.run_classification(human, task.id)
    history = app.classification.get(human, ws, task.key).history
    assert {h.validation_status for h in history} == {"invalid_output", "provider_unavailable"}
    assert app.classification.get(human, ws, task.key).current is None
    assert app.tasks.get(human, ws, task.key).status == "backlog"  # tracking unaffected


def test_classification_job_dead_letters_when_model_unavailable(app, human, ws):
    config = app.config
    config.llm.providers["ollama"] = OllamaProviderConfig(base_url=f"http://127.0.0.1:{free_port()}", max_retries=0,
                                                          timeout_seconds=1)
    app.config_store.save_global(config)
    task = app.tasks.create(human, ws, TaskCreate(title="Anything"))
    result = app.classification.request(human, ws, task.key)
    with app.db.session(write=True) as session:
        from workgraph.persistence.orm import Job

        job = session.get(Job, result["job_id"])
        job.max_attempts = 1
    drain(app)
    job = app.queue.get(result["job_id"])
    assert job.status == "dead" and job.error_kind == "provider_unavailable"


def test_heuristic_provider_reclassify_and_evaluation(app, human, ws):
    config = app.config
    config.llm.classification = ModelSelection(provider="heuristic", model="heuristic-v1")
    app.config_store.save_global(config)
    samples = [
        ("Fix crash in checkout page", "bug_fix"),
        ("Add dark mode feature", "feature"),
        ("Refactor payment module", "refactor"),
        ("Update README documentation", "documentation"),
        ("Investigate slow query latency", "performance"),
    ]
    keys = []
    for title, label in samples:
        t = app.tasks.create(human, ws, TaskCreate(title=title))
        keys.append(t.key)
        app.classification.add_gold(human, ws, task_ref=t.key, category=label)
    batch = app.classification.reclassify(human, ws)
    assert batch["tasks"] == 5
    drain(app)
    assert all(app.classification.get(human, ws, k).current for k in keys)
    evaluation = app.classification.evaluate(human, ws, provider="heuristic", model="heuristic-v1")
    assert evaluation["sample_size"] == 5
    assert evaluation["accuracy"] is not None and evaluation["accuracy"] >= 0.6
    assert set(evaluation["confusion"]) == {"bug_fix", "feature", "refactor", "documentation", "performance"}
    assert app.classification.evaluations(human, ws)[0]["id"] == evaluation["id"]


def test_search_lexical_semantic_hybrid_and_similar(app, human, ws, make_repo):
    config = app.config
    config.llm.embeddings = ModelSelection(provider="heuristic", model="hash-embed-256")
    app.config_store.save_global(config)
    repo = make_repo("auth")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    t1 = app.tasks.create(human, ws, TaskCreate(title="Login token refresh fails", description="OAuth token expiry",
                                                repositories=["auth"]))
    t2 = app.tasks.create(human, ws, TaskCreate(title="Token refresh race condition", description="oauth token renew",
                                                repositories=["auth"]))
    app.tasks.create(human, ws, TaskCreate(title="Dark mode colours", description="css theme"))
    # Full-text search works without any embeddings.
    result = app.search.search(human, ws, "token refresh")
    assert result.mode == "lexical" and not result.semantic_available
    assert {h.key for h in result.hits} == {t1.key, t2.key}
    for t in (t1, t2):
        app.search.embed_task(t.id)
    assert app.search.embed_task(t1.id)["status"] == "unchanged"
    hybrid = app.search.search(human, ws, "oauth token")
    assert hybrid.mode == "hybrid" and hybrid.semantic_available
    assert hybrid.hits[0].key in {t1.key, t2.key}
    filtered = app.search.search(human, ws, "token", mode="lexical",
                                 filters=__import__("workgraph.search.service", fromlist=["SearchFilters"]).SearchFilters(repository="auth"))
    assert len(filtered.hits) == 2
    similar = app.search.similar(human, ws, t1.key)
    assert similar["method"] == "semantic" and similar["similar"][0]["key"] == t2.key
    assert similar["similar"][0]["repository_overlap"] == 1.0
