from __future__ import annotations

import json
import sys
from dataclasses import replace
from typing import Any

import pytest
from typer.testing import CliRunner

from tests.conftest import GitRepo
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.models.schemas import RepositoryRegister, TaskFilter
from workgraph.experiments import replay

GITIGNORE = ".pytest_cache/\n__pycache__/\n"
PYPROJECT = '[project]\nname = "demo"\nversion = "0.1.0"\n'
MOD_BASE = "def f():\n    return 1\n"
MOD_WITH_G = "def f():\n    return 1\n\n\ndef g():\n    return 2\n"
MOD_WITH_H = MOD_WITH_G + "\n\ndef h():\n    return 3\n"
TEST_F = "from pkg.mod import f\n\n\ndef test_f():\n    assert f() == 1\n"
TEST_G = "from pkg.mod import g\n\n\ndef test_g():\n    assert g() == 2\n"
TEST_G_EXTRA = "from pkg.mod import g\n\n\ndef test_g_extra():\n    assert g() == 2\n"
TEST_F_AGAIN = "from pkg.mod import f\n\n\ndef test_f_again():\n    assert f() == 1\n"
TEMPLATE = [sys.executable, "-m", "pytest", "-q", "{test_files}"]


@pytest.fixture
def project(app, human, ws, make_repo) -> tuple[GitRepo, dict[str, str]]:
    """A Python repository with one eligible commit, one tests-only commit and one whose test is green at the parent."""
    repo = make_repo("pool")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    shas: dict[str, str] = {}
    shas["project"] = repo.commit(
        "add project",
        {
            "pyproject.toml": PYPROJECT,
            ".gitignore": GITIGNORE,
            "pkg/__init__.py": "",
            "pkg/mod.py": MOD_BASE,
            "tests/test_mod.py": TEST_F,
        },
    )
    shas["eligible"] = repo.commit("add g", {"pkg/mod.py": MOD_WITH_G, "tests/test_g.py": TEST_G})
    shas["tests_only"] = repo.commit("more g tests", {"tests/test_g_extra.py": TEST_G_EXTRA})
    shas["green"] = repo.commit(
        "add h and an already-green test", {"pkg/mod.py": MOD_WITH_H, "tests/test_f_again.py": TEST_F_AGAIN}
    )
    return repo, shas


def _replay_tasks(app, human, ws) -> list:
    return app.tasks.list(human, ws, TaskFilter(tags=["replay"])).items


def test_scan_returns_eligible_commit_and_hides_excluded(app, human, ws, project):
    _repo, shas = project
    found = [c.sha for c in app.replay_pool.scan(human, ws, "pool")]
    assert shas["eligible"] in found
    assert shas["tests_only"] not in found

    everything = app.replay_pool.scan(human, ws, "pool", include_excluded=True, min_lines=0)
    tests_only = next(c for c in everything if c.sha == shas["tests_only"])
    assert tests_only.excluded == ("no_source",)


def test_create_with_validate_makes_one_task_with_metadata(app, human, ws, project):
    repo, shas = project
    results = app.replay_pool.create(
        human, ws, "pool", [shas["eligible"]], validate=True, oracle_template=TEMPLATE
    )
    assert len(results) == 1
    result = results[0]
    assert result["status"] == "created"
    assert result["reason"] is None

    task = app.tasks.get(human, ws, result["task_key"])
    assert set(task.tags) == {"replay", "experiment-pool"}
    assert task.title.startswith(f"Replay {shas['eligible'][:12]}: ")
    meta = task.metadata["replay"]
    assert meta["sha"] == shas["eligible"]
    assert meta["parent"] == shas["project"]
    assert meta["base_ref"] == f"wg/replay/{shas['eligible'][:12]}"
    assert meta["test_paths"] == ["tests/test_g.py"]
    assert meta["source_paths"] == ["pkg/mod.py"]
    assert meta["oracle"] == [[*TEMPLATE[:-1], "tests/test_g.py"]]
    assert meta["features"]["src_files"] == 1
    assert meta["validated"] is True
    assert meta["validation"]["base"] and all(code != 0 for code in meta["validation"]["base"])
    assert meta["validation"]["solution"] == [0]
    assert meta["validated_at"]
    assert "def g" not in task.description

    branch = repo.git("rev-parse", "--verify", f"refs/heads/{meta['base_ref']}")
    assert len(branch) == 40


def test_create_again_reports_existing_task(app, human, ws, project):
    _, shas = project
    first = app.replay_pool.create(human, ws, "pool", [shas["eligible"]], oracle_template=TEMPLATE)[0]
    assert first["status"] == "created"
    again = app.replay_pool.create(human, ws, "pool", [shas["eligible"]], oracle_template=TEMPLATE)[0]
    assert again["status"] == "exists"
    assert again["task_key"] == first["task_key"]
    assert len(_replay_tasks(app, human, ws)) == 1


def test_green_at_base_is_rejected_without_task(app, human, ws, project):
    _, shas = project
    rejected = app.replay_pool.create(
        human, ws, "pool", [shas["green"]], validate=True, oracle_template=TEMPLATE
    )[0]
    assert rejected["status"] == "rejected"
    assert rejected["reason"] == "oracle_green_at_base"
    assert rejected["task_key"] is None
    assert _replay_tasks(app, human, ws) == []


def test_tests_only_commit_is_skipped_as_no_source(app, human, ws, project):
    _, shas = project
    skipped = app.replay_pool.create(human, ws, "pool", [shas["tests_only"]], oracle_template=TEMPLATE)[0]
    assert skipped["status"] == "skipped"
    assert skipped["reason"] == "no_source"
    assert skipped["task_key"] is None


def test_validate_passes_env_to_oracle(project):
    repo, shas = project
    candidate = replay.analyze_commit(repo.path, shas["eligible"])
    oracle_script = (
        "import os, sys; "
        "sys.exit(0 if os.environ.get('WG_ORACLE_OK') == '1' and 'def g' in open('pkg/mod.py').read() else 1)"
    )
    candidate = replace(candidate, oracle=((sys.executable, "-c", oracle_script),))
    base_ref = replay.create_base(repo.path, candidate)

    with_env = replay.validate(repo.path, candidate, base_ref, env={"WG_ORACLE_OK": "1"})
    assert with_env.ok, with_env
    without_env = replay.validate(repo.path, candidate, base_ref)
    assert not without_env.ok
    assert without_env.reason == "oracle_red_at_solution"


def test_cli_scan_and_create(home, monkeypatch, tmp_path, project):
    repo, shas = project
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude-config"))
    runner = CliRunner()

    def invoke(*args: str) -> Any:
        STATE.json = True
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == 0, f"{args}: exit {result.exit_code}\n{result.output}"
        return json.loads(result.output.strip())

    invoke("init")
    invoke("repo", "register", str(repo.path))

    scanned = invoke("experiment", "replay", "scan", "--repo", "pool")
    assert shas["eligible"] in [row["sha"] for row in scanned]
    assert all(row["sha"] != shas["tests_only"] for row in scanned)

    template = json.dumps(TEMPLATE)
    created = invoke(
        "experiment", "replay", "create", "--repo", "pool", shas["eligible"], "--oracle-template", template
    )
    assert [r["status"] for r in created] == ["created"]
    again = invoke(
        "experiment", "replay", "create", "--repo", "pool", shas["eligible"], "--oracle-template", template
    )
    assert [r["status"] for r in again] == ["exists"]
    assert again[0]["task_key"] == created[0]["task_key"]
