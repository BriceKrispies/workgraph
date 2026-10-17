from __future__ import annotations

import json
import os
import subprocess
import sys

import pytest
from typer.testing import CliRunner

from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli


@pytest.fixture
def run(home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    runner = CliRunner()

    def invoke(*args: str, expect: int = 0) -> object:
        STATE.json = True
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        text = result.output.strip()
        return json.loads(text) if text.startswith(("{", "[")) else text

    return invoke


def test_cli_workflow_json(run, make_repo):
    init = run("init", "--workspace", "acme", "--prefix", "AC")
    assert init["workspace_created"] == "acme"
    repo = make_repo("svc")
    reg = run("repo", "register", str(repo.path))
    assert reg["name"] == "svc"
    run("project", "create", "core", "--repo", "svc")
    task = run(
        "task", "create", "Fix crash in svc", "--ready", "--repo", "svc", "--project", "core", "-t", "bug"
    )
    assert task["key"] == "AC-1"
    child = run("task", "create", "Child", "--parent", "AC-1", "--depends-on", "AC-1", "--ready")
    assert child["status"] == "blocked"
    ready = run("task", "ready")
    assert [t["key"] for t in ready] == ["AC-1"]
    listing = run("task", "list", "--status", "ready", "--tag", "bug")
    assert listing["total"] == 1
    assert run("task", "claim", "AC-1")["status"] == "claimed"
    assert run("task", "complete", "AC-1", "-m", "fixed")["status"] == "awaiting_review"
    accepted = run("task", "accept", "AC-1", "--manual", "--reason", "reviewed")
    assert accepted["mode"] == "manual"
    assert run("task", "show", "AC-2")["status"] == "ready"
    history = run("task", "history", "AC-1")
    assert history[0]["event_type"] == "task.created"
    sha = repo.commit("work\n\nTask-ID: AC-2", {"x.py": "x = 1\n"})
    sync = run("git", "sync", "svc")
    assert sync[0]["new_commits"] >= 1
    changes = run("git", "changes", "AC-2")
    assert changes["commits"][0]["sha"] == sha
    graph = run("task", "graph")
    assert len(graph["edges"]) == 2
    search = run("search", "crash", "--mode", "lexical")
    assert search["hits"][0]["key"] == "AC-1"
    metrics = run("analytics", "query", "tasks_created", "--by", "status")
    assert {r["status"] for r in metrics["rows"]} == {"accepted", "ready"}
    assert run("events", "verify")["consistent"] is True
    assert run("config", "validate")["valid"] is True
    run("config", "set", "execution.max_concurrent_runs", "4")
    assert run("config", "show")["config"]["execution"]["max_concurrent_runs"] == 4
    run("config", "set", "execution.max_concurrent_runs", '"lots"', expect=7)


def test_cli_errors_and_exit_codes(run):
    run("init")
    err = run("task", "show", "WG-404", expect=4)
    assert err["error"]["code"] == "not_found"
    run("task", "create", "A")
    run("task", "update", "WG-1", "--status", "accepted", expect=3)
    run("task", "depend", "WG-1", "WG-1", expect=2)


def test_cli_backup_restore_export_import(run, tmp_path, home):
    run("init")
    run("task", "create", "Keep me", "--description", "important")
    backup = run("backup", "--output", str(tmp_path / "b.zip"))
    assert backup["schema_revision"]
    run("task", "create", "Created after backup")
    assert run("task", "list")["total"] == 2
    restored = run("restore", str(tmp_path / "b.zip"))
    assert restored["safety_copy"]
    assert run("task", "list")["total"] == 1
    exported = run("export", "workspace", str(tmp_path / "ws.jsonl"))
    assert exported["tables"]["tasks"] == 1
    other_home = tmp_path / "other"
    env = {**os.environ, "WORKGRAPH_HOME": str(other_home)}
    subprocess.run(
        [sys.executable, "-m", "workgraph", "init", "--no-workspace"],
        env=env,
        check=True,
        capture_output=True,
    )
    imported = subprocess.run(
        [sys.executable, "-m", "workgraph", "--json", "import", str(tmp_path / "ws.jsonl")],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert imported.returncode == 0, imported.stderr
    tasks = subprocess.run(
        [sys.executable, "-m", "workgraph", "--json", "-w", "default", "task", "list"],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    data = json.loads(tasks.stdout)
    assert data["total"] == 1 and data["items"][0]["title"] == "Keep me"
    search = subprocess.run(
        [
            sys.executable,
            "-m",
            "workgraph",
            "--json",
            "-w",
            "default",
            "search",
            "important",
            "--mode",
            "lexical",
        ],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert json.loads(search.stdout)["hits"]
    again = subprocess.run(
        [sys.executable, "-m", "workgraph", "--json", "import", str(tmp_path / "ws.jsonl")],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert again.returncode == 3


def test_doctor_reports_health(run):
    run("init")
    report = run("doctor", expect=0)
    names = {c["check"] for c in report["checks"]}
    assert {"python", "git", "schema", "database integrity", "configuration"} <= names
    assert report["healthy"] is True


def test_console_script_help():
    proc = subprocess.run(
        [sys.executable, "-m", "workgraph", "--help"],
        capture_output=True,
        encoding="utf-8",
        env={**os.environ, "PYTHONIOENCODING": "utf-8"},
        check=False,
    )
    assert proc.returncode == 0 and "task" in proc.stdout and "analytics" in proc.stdout
