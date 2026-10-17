from __future__ import annotations

import json
import sys

import pytest
from typer.testing import CliRunner

from tests.helpers import FAKE_AGENT
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli


@pytest.fixture
def run(home, monkeypatch, tmp_path):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude-config"))
    runner = CliRunner()

    def invoke(*args: str, expect: int = 0, json_mode: bool = True) -> object:
        STATE.json = json_mode
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        text = result.output.strip()
        if json_mode and text.startswith(("{", "[")):
            return json.loads(text)
        return text

    return invoke


def test_all_command_groups(run, make_repo, tmp_path):
    run("init")
    run("config", "set", "llm.classification", '{"provider": "heuristic", "model": "heuristic-v1"}')
    run("config", "set", "llm.embeddings", '{"provider": "heuristic", "model": "hash-embed-256"}')
    run("config", "set", "llm.auto_classify", "false")
    run("config", "set", "llm.auto_embed", "false")
    repo = make_repo("svc")
    run("repo", "register", str(repo.path))
    run("repo", "show", "svc")
    run("repo", "status", "svc")
    run("repo", "refresh", "svc")
    run("repo", "list")
    run("workspace", "list")
    run("workspace", "show")
    run("project", "create", "p", "--repo", "svc")
    run("project", "list")
    run("project", "show", "p")
    run("project", "update", "p", "--description", "x")
    # agents and runs
    run("config", "set", "execution.enabled", "true")
    run(
        "agent",
        "add-command",
        "fake",
        sys.executable,
        "--arg",
        FAKE_AGENT,
        "--arg",
        "{manifest}",
        "--supports-usage",
    )
    agents = run("agent", "list")
    assert any(a["name"] == "fake" and a["available"] for a in agents)
    run("task", "create", "Fix crash in login", "--ready", "--repo", "svc", "--project", "p")
    run(
        "verify",
        "config",
        "ci",
        "--scope-type",
        "repository",
        "--scope",
        "svc",
        "--check",
        json.dumps({"name": "ok", "command": [sys.executable, "-c", "print(1)"]}),
    )
    run("verify", "configs")
    attempt = run("run", "start", "WG-1", "--agent", "fake")
    run("worker", "run", "--once")
    run("worker", "list")
    run("run", "get", attempt["id"])
    run("attempt", "list")
    run("attempt", "show", attempt["id"])
    run("attempt", "manifests", attempt["id"])
    run("verify", "runs")
    run("verify", "failures")
    run("verify", "run", "WG-1")
    run("task", "accept", "WG-1")
    run("job", "list")
    run("job", "stats")
    job = run("job", "enqueue", "noop")
    run("job", "show", job["job_id"])
    run("job", "cancel", job["job_id"])
    run("job", "retry", job["job_id"])
    run("job", "compact")
    run("worker", "recover")
    # git
    run("git", "sync")
    run("git", "log", "svc")
    changes = run("git", "changes", "WG-1")
    sha = changes["commits"][0]["sha"]
    run("git", "show", "svc", sha)
    run("git", "diff", "WG-1")
    run("git", "surface", "WG-1")
    run("git", "analyze", "svc")
    run("git", "worktrees")
    run("git", "cleanup-worktrees")
    run("git", "link", "WG-1", sha)
    run("git", "unlink", "WG-1", sha)
    # classification and search
    run("classify", "request", "WG-1", "--now")
    run("classify", "show", "WG-1")
    run("classify", "confirm", "WG-1", "bug_fix")
    run("classify", "gold")
    run("classify", "evaluate", "--provider", "heuristic", "--model", "heuristic-v1")
    run("classify", "evaluations")
    run("classify", "disagreements")
    run("classify", "reclassify")
    run("search", "crash")
    run("search", "x", "--similar-to", "WG-1")
    run("search", "x", "--reindex")
    # analytics
    run("analytics", "metrics")
    run("analytics", "overview")
    run("analytics", "coupling")
    run("analytics", "failures", "--persist")
    run("analytics", "history")
    run("analytics", "estimate", "WG-1")
    run("analytics", "export", str(tmp_path / "f.csv"), "--dataset", "task_facts")
    run("analytics", "retrospective", "--no-model", "--output", str(tmp_path / "retro.md"))
    assert (tmp_path / "retro.md").exists()
    run("analytics", "compare", "tasks_created", "2026-01-01", "2026-06-01", "2026-06-01", "2027-01-01")
    # automation
    rule = {"name": "r1", "trigger": {"type": "manual"}, "actions": [{"type": "add_tags", "tags": ["auto"]}]}
    run("automation", "create", json.dumps(rule))
    run("automation", "list")
    run("automation", "show", "r1")
    run("automation", "preview", "--rule", "r1", "--task", "WG-1")
    run("automation", "trigger", "r1", "--task", "WG-1")
    run("automation", "process")
    run("automation", "runs")
    run("automation", "disable", "r1")
    run("automation", "enable", "r1")
    run("automation", "update", "r1", json.dumps(rule))
    run("automation", "notifications")
    run("automation", "catalog")
    # claude integration
    run("claude", "install", "--command", str(tmp_path / "workgraph.exe"))
    status = run("claude", "status")
    assert status["installation"]["missing_events"] == []
    run("claude", "bind", "WG-1", "--session", "abc")
    run("claude", "sessions")
    run("claude", "replay")
    run("claude", "unbind", "--cwd", str(tmp_path))
    run("claude", "mcp-config", "--command", "workgraph")
    run("claude", "uninstall")
    # events, export
    run("events", "list")
    run("export", "workspace", str(tmp_path / "ws.jsonl"))
    run("export", "analytics", str(tmp_path / "a.json"), "--format", "json")
    # human-readable output paths
    for args in (
        ("task", "list"),
        ("task", "show", "WG-1"),
        ("repo", "list"),
        ("analytics", "query", "tasks_created"),
        ("job", "list"),
        ("automation", "list"),
        ("git", "log", "svc"),
        ("doctor",),
    ):
        out = run(*args, json_mode=False)
        assert isinstance(out, str) and out


def test_stack_command_group(run, make_repo):
    run("init")
    run("config", "set", "llm.auto_classify", "false")
    run("config", "set", "llm.auto_embed", "false")
    repo = make_repo("stacked")
    run("repo", "register", str(repo.path))
    run("task", "create", "Parent work", "--repo", "stacked")
    run("task", "create", "First change", "--parent", "WG-1", "--repo", "stacked")
    run("task", "create", "Second change", "--parent", "WG-1", "--repo", "stacked")
    created = run("stack", "create", "WG-1")
    assert created["parent_key"] == "WG-1"
    assert created["repositories"][0]["base_sha"] == repo.head()
    assert [i["state"] for i in created["items"]] == ["pending", "pending"]
    shown = run("stack", "show", "WG-1")
    assert shown["id"] == created["id"]
    assert [s["parent_key"] for s in run("stack", "list")] == ["WG-1"]
    assert run("stack", "list", "--all")[0]["id"] == created["id"]
    assert "WG-1" in run("stack", "show", "WG-1", json_mode=False)
    assert "WG-1" in run("stack", "list", json_mode=False)
    run("stack", "show", "WG-9", expect=4)
