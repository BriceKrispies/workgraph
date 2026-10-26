from __future__ import annotations

import json

import pytest
from typer.testing import CliRunner

from tests.helpers import run_cli, run_cli_process
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli


@pytest.fixture
def run(home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))

    def invoke(*args: str, expect: int = 0) -> object:
        result = run_cli(["--json", *args])
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


def test_task_tags_accept_comma_separated_values(run):
    run("init", "--workspace", "acme", "--prefix", "AC")
    task = run("task", "create", "Retro it", "-t", "rho,retro", "-t", " bug , ,")
    assert sorted(task["tags"]) == ["bug", "retro", "rho"]
    updated = run("task", "update", "AC-1", "--add-tag", "ops,ci", "--remove-tag", "bug, ")
    assert sorted(updated["tags"]) == ["ci", "ops", "retro", "rho"]


def test_repo_update_tracks_and_untracks_branches(run, make_repo):
    run("init", "--workspace", "acme", "--prefix", "AC")
    repo = make_repo("svc")
    run("repo", "register", str(repo.path))

    out = run("repo", "update", "svc", "--track", "release", "--track", "main", "--track", "feature")
    assert out["tracked_branches"] == ["main", "release", "feature"]

    out = run("repo", "update", "svc", "--untrack", "release", "--track", "hotfix")
    assert out["tracked_branches"] == ["main", "feature", "hotfix"]


def test_repo_update_default_branch_change_tracks_the_new_default(run, make_repo):
    run("init", "--workspace", "acme", "--prefix", "AC")
    repo = make_repo("svc")
    run("repo", "register", str(repo.path))
    run("repo", "update", "svc", "--track", "feature")
    repo.git("branch", "develop")

    out = run("repo", "update", "svc", "--default-branch", "develop")

    assert out["default_branch"] == "develop"
    assert out["tracked_branches"] == ["main", "feature", "develop"]
    assert run("config", "validate")["valid"] is True
    run("config", "set", "execution.max_concurrent_runs", "4")
    assert run("config", "show")["config"]["execution"]["max_concurrent_runs"] == 4
    run("config", "set", "execution.max_concurrent_runs", '"lots"', expect=7)


def test_cli_attempt_start_json_keys(run, make_repo):
    run("init", "--workspace", "acme", "--prefix", "AC")
    repo = make_repo("svc")
    run("repo", "register", str(repo.path))
    task = run("task", "create", "External work", "--ready", "--repo", "svc")

    out = run("attempt", "start", task["key"], "--agent-name", "tester")

    assert set(out) == {"attempt_id", "attempt_number", "task_key", "repositories", "agent_checks"}
    assert out["task_key"] == task["key"] and out["attempt_number"] == 1
    [repository] = out["repositories"]
    assert set(repository) == {"repository", "checkout_path", "branch", "base_sha", "base"}
    assert repository["repository"] == "svc" and repository["base_sha"] == repo.head()
    assert repository["branch"].startswith(f"wg/{task['key'].lower()}/attempt-1-")
    assert run("task", "show", task["key"])["status"] == "running"


def test_cli_attempt_submit_json_keys(run, make_repo):
    run("init", "--workspace", "acme", "--prefix", "AC")
    repo = make_repo("svc")
    run("repo", "register", str(repo.path))
    task = run("task", "create", "External work", "--ready", "--repo", "svc")
    started = run("attempt", "start", task["key"], "--agent-name", "tester")

    out = run("attempt", "submit", started["attempt_id"], "--failed", "--error", "tests are red")

    assert out["status"] == "failed" and out["error"] == "tests are red"
    assert out["task_status"] == "failed" and out["attempt_status"] == "failed"
    assert run("task", "show", task["key"])["status"] == "failed"


def test_cli_errors_and_exit_codes(run):
    run("init")
    err = run("task", "show", "WG-404", expect=4)
    assert err["error"]["code"] == "not_found"
    run("task", "create", "A")
    run("task", "update", "WG-1", "--status", "accepted", expect=3)
    run("task", "depend", "WG-1", "WG-1", expect=2)


def test_task_list_limit_above_maximum_is_a_validation_error(run, home, monkeypatch):
    run("init")
    err = run("task", "list", "--limit", "2000", expect=2)
    assert err["error"]["code"] == "validation_failed"
    assert "500" in err["error"]["message"]
    assert any(item["loc"] == ["limit"] for item in err["error"]["details"]["errors"])

    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    try:
        result = CliRunner().invoke(cli, ["task", "list", "--limit", "2000"])
    finally:
        STATE.close()  # the app opened here would otherwise serve the next test's home
    assert result.exit_code == 2, result.output
    assert "validation_failed" in result.output
    assert "limit" in result.output
    assert "Traceback" not in result.output


def test_cli_task_show_single_ref_shape(run):
    run("init")
    run("task", "create", "Solo")
    shown = run("task", "show", "WG-1")
    assert shown["key"] == "WG-1" and "items" not in shown


def test_cli_task_show_several_refs(run, monkeypatch):
    run("init")
    run("task", "create", "First")
    run("task", "create", "Second")
    run("task", "comment", "WG-1", "first note")
    run("task", "comment", "WG-2", "second note")
    single = run("task", "show", "WG-1")

    opened = []
    original_app = STATE.app
    monkeypatch.setattr(STATE, "app", lambda: opened.append(1) or original_app())
    many = run("task", "show", "WG-2", "WG-1")

    assert len(opened) == 1
    assert set(many) == {"items", "errors"}
    assert [item["key"] for item in many["items"]] == ["WG-2", "WG-1"]
    assert many["items"][1] == single
    assert [c["body"] for c in many["items"][1]["comments"]] == ["first note"]
    assert [c["body"] for c in many["items"][0]["comments"]] == ["second note"]
    assert many["errors"] == []


def test_cli_task_show_unknown_ref_among_several(run):
    run("init")
    run("task", "create", "Known")
    out = run("task", "show", "WG-404", "WG-1", expect=4)
    assert [item["key"] for item in out["items"]] == ["WG-1"]
    assert out["errors"][0]["ref"] == "WG-404"
    assert out["errors"][0]["error"]["code"] == "not_found"


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
    env = {"WORKGRAPH_HOME": str(other_home)}
    init = run_cli(["init", "--no-workspace"], env=env)
    assert init.exit_code == 0, init.stderr
    imported = run_cli(["--json", "import", str(tmp_path / "ws.jsonl")], env=env)
    assert imported.exit_code == 0, imported.stderr
    tasks = run_cli(["--json", "-w", "default", "task", "list"], env=env)
    data = json.loads(tasks.stdout)
    assert data["total"] == 1 and data["items"][0]["title"] == "Keep me"
    search = run_cli(["--json", "-w", "default", "search", "important", "--mode", "lexical"], env=env)
    assert json.loads(search.stdout)["hits"]
    again = run_cli(["--json", "import", str(tmp_path / "ws.jsonl")], env=env)
    assert again.exit_code == 3


def test_doctor_reports_health(run):
    run("init")
    report = run("doctor", expect=0)
    names = {c["check"] for c in report["checks"]}
    assert {"python", "git", "schema", "database integrity", "configuration"} <= names
    assert report["healthy"] is True


def test_task_show_survives_cp1252_console(run, home):
    # A cp1252 console cannot encode arrows, em dashes or ≤; the CLI must replace them, not crash.
    run("init")
    run("task", "create", "Unicode task", "--description", "step a → b — c ≤ d")
    env = {
        "WORKGRAPH_HOME": str(home),
        "PYTHONIOENCODING": "cp1252",
        "PYTHONUTF8": "0",
    }
    proc = run_cli_process(["-w", "default", "task", "show", "WG-1"], env=env)
    assert proc.exit_code == 0, proc.stderr.decode("ascii", "replace")
    assert b"Unicode task" in proc.stdout


def _piped_cli(home, *args: str):
    # stdout is a pipe here, the case that crashed rich's legacy Windows path.
    env = {
        "FORCE_COLOR": None,
        "COLUMNS": None,
        "WORKGRAPH_HOME": str(home),
        "PYTHONUTF8": "1",
        "NO_COLOR": "1",
    }
    return run_cli_process(["-w", "default", *args], env=env)


def test_piped_human_output_renders_as_plain_text(run, home):
    run("init")
    run("task", "create", "Piped task", "--ready", "--description", "body text")
    show = _piped_cli(home, "task", "show", "WG-1")
    assert show.exit_code == 0, show.stderr.decode("utf-8", "replace")
    assert b"Traceback" not in show.stderr
    assert b"Piped task" in show.stdout
    assert b"\x1b[" not in show.stdout
    listing = _piped_cli(home, "task", "list")
    assert listing.exit_code == 0, listing.stderr.decode("utf-8", "replace")
    assert b"WG-1" in listing.stdout and b"Piped task" in listing.stdout


def test_cli_console_skips_legacy_windows_path_off_a_terminal():
    import io

    from workgraph.cli.common import CliConsole

    stream = io.StringIO()
    console = CliConsole(file=stream, highlight=False)
    console.legacy_windows = True  # what rich reports on a Windows console without VT
    assert console.legacy_windows is False
    console.print("[bold]task[/bold] ok")
    assert stream.getvalue() == "task ok\n"


def test_console_script_help():
    proc = run_cli(["--help"], env={"PYTHONIOENCODING": "utf-8"})
    assert proc.exit_code == 0 and "task" in proc.stdout and "analytics" in proc.stdout


def test_promote_ready_cli_refuses_without_a_scope(run):
    run("init", "--workspace", "acme", "--prefix", "AC")
    run("task", "create", "Plain")  # AC-1
    for args in ([], ["--all", "--project", "core"], ["--all", "--tag", "sprint"]):
        refused = run("task", "promote-ready", *args, expect=2)
        assert refused["error"]["code"] == "validation_failed"
    assert run("task", "show", "AC-1")["status"] == "backlog"


def test_promote_ready_cli_counts_per_project(run):
    run("init", "--workspace", "acme", "--prefix", "AC")
    run("project", "create", "core")
    run("project", "create", "ops")
    run("task", "create", "Both", "--project", "core", "--project", "ops")  # AC-1
    run("task", "create", "Core", "--project", "core")  # AC-2
    run("task", "create", "Unfiled")  # AC-3
    assert run("task", "promote-ready", "--project", "core", "--dry-run") == {
        "promoted": ["AC-1", "AC-2"],
        "by_project": {"core": 2, "ops": 1},
        "dry_run": True,
    }
    assert run("task", "show", "AC-1")["status"] == "backlog"
    assert run("task", "promote-ready", "--all") == {
        "promoted": ["AC-1", "AC-2", "AC-3"],
        "by_project": {"(none)": 1, "core": 2, "ops": 1},
        "dry_run": False,
    }


def test_promote_ready_cli_and_accept_promote_dependents(run):
    run("init", "--workspace", "acme", "--prefix", "AC")
    run("task", "create", "Base", "--ready")  # AC-1
    run("task", "create", "Follow", "--depends-on", "AC-1")  # AC-2
    run("task", "create", "Plain")  # AC-3
    assert run("task", "promote-ready", "--all", "--dry-run") == {
        "promoted": ["AC-3"],
        "by_project": {"(none)": 1},
        "dry_run": True,
    }
    assert run("task", "show", "AC-3")["status"] == "backlog"
    assert run("task", "promote-ready", "--all") == {
        "promoted": ["AC-3"],
        "by_project": {"(none)": 1},
        "dry_run": False,
    }
    assert run("task", "show", "AC-3")["status"] == "ready"
    run("task", "claim", "AC-1")
    run("task", "complete", "AC-1", "-m", "done")
    accepted = run("task", "accept", "AC-1", "--manual", "--reason", "reviewed", "--promote-dependents")
    assert accepted["mode"] == "manual"
    assert accepted["promoted"] == ["AC-2"]
    assert run("task", "show", "AC-2")["status"] == "ready"
