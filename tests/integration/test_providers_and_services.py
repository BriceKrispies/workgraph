from __future__ import annotations

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest
from sqlalchemy import select

from tests.helpers import drain
from workgraph.config.settings import ClaudeCodeAgentConfig, ModelSelection, OpenAICompatibleProviderConfig
from workgraph.core.errors import ConflictError, DependencyCycleError, ValidationFailed
from workgraph.core.models.schemas import (
    ProjectCreate,
    ProjectUpdate,
    RepositoryRegister,
    RepositoryUpdate,
    TaskCreate,
    WorkspaceCreate,
)
from workgraph.integrations.llm.base import InvalidModelOutputError
from workgraph.integrations.llm.heuristic import classify_text, hash_embedding
from workgraph.integrations.llm.openai_compatible import OpenAICompatibleProvider
from workgraph.orchestration.scheduler import ChildProposal, TaskDecomposition, validate_decomposition
from workgraph.persistence.orm import AgentSession

FAKE_CLAUDE = Path(__file__).parent.parent / "fixtures" / "fake_claude.py"


def fake_claude_executable(tmp_path: Path) -> str:
    if os.name == "nt":
        wrapper = tmp_path / "claude.cmd"
        wrapper.write_text(f'@"{sys.executable}" "{FAKE_CLAUDE}" %*\r\n', encoding="utf-8")
    else:
        wrapper = tmp_path / "claude"
        wrapper.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE_CLAUDE}" "$@"\n', encoding="utf-8")
        wrapper.chmod(0o755)
    return str(wrapper)


def test_claude_code_adapter_stream_json(app, human, ws, make_repo, tmp_path, monkeypatch):
    exe = fake_claude_executable(tmp_path)
    config = app.config
    config.execution.enabled = True
    config.agents["claude_code"] = ClaudeCodeAgentConfig(
        executable=exe, model="claude-test", allowed_tools=["Edit"]
    )
    app.config_store.save_global(config)
    provider = app.agents.get("claude_code")
    ok, detail = provider.check_available()
    assert ok and "Claude Code" in detail
    assert provider.capabilities().usage_reporting and provider.capabilities().lifecycle_hooks
    repo = make_repo("cc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(human, ws, TaskCreate(title="Use Claude", repositories=["cc"], status="ready"))
    attempt = app.runs.start(human, ws, task.key, agent="claude_code")
    drain(app)
    out = app.attempts.get(human, attempt.id)
    assert out.status == "succeeded", out.error
    assert out.usage["input_tokens"] == 900 and out.usage["num_turns"] == 3
    assert out.usage_source == "claude_code_reported"
    assert out.cost_usd == pytest.approx(0.0123) and out.cost_source == "claude_code_reported"
    assert out.telemetry["tool_uses"] == 2 and out.telemetry["tool_errors"] == 1
    assert out.telemetry["subagent_launches"] == 1
    assert out.telemetry["provider_reported_duration_ms"] == 4321
    assert out.session_id and out.model == "claude-test"
    with app.db.session() as session:
        bound = session.scalar(select(AgentSession).where(AgentSession.external_session_id == out.session_id))
        assert (
            bound is not None
            and bound.binding_source == "workgraph_launched"
            and bound.attempt_id == attempt.id
        )
    commits = app.git.task_commits(human, ws, task.key)
    assert len(commits) == 1
    # failure path
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "error")
    app.verification.reject(human, ws, task.key, reason="again")
    failed = app.runs.start(human, ws, task.key, agent="claude_code")
    drain(app)
    assert app.attempts.get(human, failed.id).status == "failed"


def test_claude_adapter_unavailable(app):
    config = app.config
    config.agents["claude_code"] = ClaudeCodeAgentConfig(executable="claude-not-installed-anywhere")
    app.config_store.save_global(config)
    ok, detail = app.agents.get("claude_code").check_available()
    assert not ok and "install Claude Code" in detail
    assert app.agents.health()["claude_code"]["available"] is False


class _OpenAIHandler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: Any) -> None:
        pass

    def _send(self, status: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        self._send(200, {"data": [{"id": "local-model"}]})

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if body.get("model") != "local-model":
            self._send(404, {"error": "no such model"})
        elif self.path == "/v1/chat/completions":
            prompt = body["messages"][-1]["content"]
            content = "nope" if "BROKEN" in prompt else json.dumps(classify_text(prompt))
            self._send(200, {"model": "local-model", "choices": [{"message": {"content": content}}]})
        else:
            self._send(
                200,
                {"data": [{"index": i, "embedding": hash_embedding(t)} for i, t in enumerate(body["input"])]},
            )


def test_openai_compatible_provider():
    server = ThreadingHTTPServer(("127.0.0.1", 0), _OpenAIHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}"
        provider = OpenAICompatibleProvider("lm", OpenAICompatibleProviderConfig(base_url=url, max_retries=0))
        assert provider.check_model("local-model") == (True, "local-model available")
        assert provider.check_model("other")[0] is False
        resp = provider.generate_json(
            "local-model", system="s", prompt="fix crash bug", schema={"title": "T"}
        )
        assert resp.data["category"] == "bug_fix"
        with pytest.raises(InvalidModelOutputError):
            provider.generate_json("local-model", system="s", prompt="BROKEN", schema={})
        assert len(provider.embed("local-model", ["a", "b"])) == 2
    finally:
        server.shutdown()
        server.server_close()


def test_decomposition_validation_and_creation(app, human, ws, make_repo):
    repo = make_repo("dec")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    config = app.config
    config.llm.summarization = ModelSelection(provider="heuristic", model="heuristic-v1")
    app.config_store.save_global(config)
    task = app.tasks.create(
        human,
        ws,
        TaskCreate(
            title="Big feature",
            repositories=["dec"],
            description="- Design the API\n- Then implement the endpoint\n- Then write the docs",
        ),
    )
    proposal = app.scheduler.decompose(human, task.id, create=False)
    assert len(proposal["proposal"]["children"]) == 3 and proposal["created"] == []
    created = app.scheduler.decompose(human, task.id, create=True)
    assert len(created["created"]) == 3
    children = app.tasks.detail(human, ws, task.key).child_ids
    assert len(children) == 3
    second = app.tasks.get(human, ws, created["created"][1])
    assert second.status == "blocked" and len(second.depends_on) == 1
    cyclic = TaskDecomposition(
        children=[
            ChildProposal(ref="a", title="A", depends_on=["b"]),
            ChildProposal(ref="b", title="B", depends_on=["a"]),
        ]
    )
    with pytest.raises(DependencyCycleError):
        validate_decomposition(cyclic, set())
    with pytest.raises(ValidationFailed):
        validate_decomposition(
            TaskDecomposition(children=[ChildProposal(ref="a", title="A", repositories=["x"])]), set()
        )
    with pytest.raises(ValidationFailed):
        validate_decomposition(
            TaskDecomposition(children=[ChildProposal(ref="a", title="A", depends_on=["z"])]), set()
        )


def test_workspace_and_project_services(app, human, ws, make_repo):
    with pytest.raises(ConflictError):
        app.workspaces.create(human, WorkspaceCreate(slug="main", name="dup"))
    with pytest.raises(ValidationFailed):
        app.workspaces.update_settings(human, ws, {"unknown": 1})
    app.workspaces.update_settings(human, ws, {"allowed_roots": [str(make_repo("inside").path.parent)]})
    outside = make_repo.__self__ if hasattr(make_repo, "__self__") else None
    del outside
    repo = make_repo("allowed")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    project = app.projects.create(human, ws, ProjectCreate(slug="p1", name="P1"))
    with pytest.raises(ConflictError):
        app.projects.create(human, ws, ProjectCreate(slug="p1", name="again"))
    app.projects.add_repository(human, ws, "p1", "allowed")
    summary = app.projects.summary(human, ws, "p1")
    assert summary.repositories[0].name == "allowed"
    app.projects.remove_repository(human, ws, "p1", "allowed")
    updated = app.projects.update(human, ws, "p1", ProjectUpdate(status="archived", description="done"))
    assert updated.status == "archived" and project.id == updated.id
    assert app.projects.list(human, ws) == []
    assert len(app.projects.list(human, ws, include_archived=True)) == 1
    summary_ws = app.workspaces.summary(human, ws)
    assert summary_ws.repositories == 1
    updated_repo = app.repositories.update(human, ws, "allowed", RepositoryUpdate(access_mode="read_only"))
    assert updated_repo.access_mode == "read_only"
    task = app.tasks.create(human, ws, TaskCreate(title="x", repositories=["allowed"]))
    with pytest.raises(ValidationFailed):
        app.tasks.link_repository(human, ws, task.key, "allowed", "read_write")
    archived = app.workspaces.archive(human, "main")
    assert archived.archived_at is not None


def test_repository_boundary_enforced(app, human, ws, make_repo, tmp_path):
    app.workspaces.update_settings(human, ws, {"allowed_roots": [str(tmp_path / "nowhere")]})
    repo = make_repo("outside")
    with pytest.raises(ValidationFailed):
        app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    with pytest.raises(ValidationFailed):
        app.repositories.register(human, ws, RepositoryRegister(path=str(tmp_path / "missing")))
