from __future__ import annotations

import json
import subprocess
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from tests.conftest import GitRepo
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.clock import utcnow
from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.core.models.schemas import RepositoryRegister
from workgraph.persistence.orm import AgentSession, SubagentRun

GITIGNORE = ".pytest_cache/\n__pycache__/\n"
PYPROJECT = '[project]\nname = "demo"\nversion = "0.1.0"\n'
MOD_BASE = "def f():\n    return 1\n"
MOD_WITH_G = "def f():\n    return 1\n\n\ndef g():\n    return 2\n"
MOD_G_STUB = MOD_BASE + "\n\ndef g():\n    raise NotImplementedError\n"
TEST_F = "from pkg.mod import f\n\n\ndef test_f():\n    assert f() == 1\n"
TEST_G = "from pkg.mod import g\n\n\ndef test_g():\n    assert g() == 2\n"
TEST_G_EDITED = "from pkg.mod import g\n\n\ndef test_g():\n    assert g() == 2 or True\n"
TEMPLATE = [sys.executable, "-m", "pytest", "-q", "{test_files}"]
# The worktree path is passed as an argv element, not spliced into Python source, so Windows backslashes stay literal.
SETUP = [sys.executable, "-c", "import sys; open(sys.argv[1], 'w').close()", "{worktree}/setup-ran"]
PINNED = "claude-haiku-5-5"
OLDER = "claude-haiku-4-5-20251001"


def protocol(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "arms": [{"name": "haiku", "model": PINNED}],
        "brief_template": "replay-v1",
        "primary_size": "src_lines",
        "bands": {"min": 1, "max": 5},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
        "budget": {"trials": 40},
        "oracle_template": TEMPLATE,
        "setup": [SETUP],
    }
    base.update(overrides)
    return base


@pytest.fixture
def project(app, human, ws, make_repo) -> tuple[GitRepo, dict[str, str]]:
    """A Python repository whose second commit adds a function and its test, both in one commit."""
    repo = make_repo("trials")
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
    return repo, shas


@pytest.fixture
def replay_task(app, human, ws, project) -> str:
    _, shas = project
    result = app.replay_pool.create(
        human, ws, "trials", [shas["eligible"]], validate=True, oracle_template=TEMPLATE
    )
    assert result[0]["status"] == "created", result
    return result[0]["task_key"]


@pytest.fixture
def running(app, human, ws):
    experiment = app.experiments.create(human, ws, "trial-exp", "Trial experiment", None, protocol())
    return app.experiments.freeze(human, ws, experiment.slug)


def _trial(app, human, ws, task_key: str, experiment: str = "trial-exp") -> Any:
    return app.experiments.add_trial(human, ws, experiment, task_key, "haiku", "staircase")


def _write(worktree: str | Path, rel: str, content: str) -> None:
    target = Path(worktree, rel)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8", newline="\n")


def _record_run(app, trial_id: str, model: str, transcript_path: str | None = None) -> str:
    """A subagent run whose telemetry names the trial, as the transcript ingester would record it."""
    unique = uuid.uuid4().hex
    with app.db.session(write=True) as session:
        agent_session = AgentSession(provider="claude-code", external_session_id=f"session-{unique}")
        session.add(agent_session)
        session.flush()
        run = SubagentRun(
            session_id=agent_session.id,
            external_agent_id=f"agent-{unique}",
            attempt_id="attempt-7",
            model=model,
            telemetry={"trial_ids": [trial_id]},
            transcript_path=transcript_path,
            stopped_at=utcnow(),
        )
        session.add(run)
        session.flush()
        return run.id


def test_start_builds_isolated_single_branch_clone(app, human, ws, project, replay_task, running):
    _repo, shas = project
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)

    worktree = started["worktree"]
    branch = f"wg/trial/{trial.id[:8]}"
    assert started["branch"] == branch
    assert started["trial"].status == "running"
    assert started["trial"].worktree_path == worktree
    assert started["trial"].base_ref == started["base_sha"]

    clone = GitRepo(Path(worktree))
    assert clone.head() == started["base_sha"]
    base_ref = app.tasks.get(human, ws, replay_task).metadata["replay"]["base_ref"]
    assert set(clone.git("branch", "-a", "--format=%(refname:short)").split()) == {branch, base_ref}
    assert clone.git("remote") == ""
    assert shas["eligible"] not in clone.git("rev-list", "--all").split()
    assert Path(worktree, "pkg", "mod.py").read_text(encoding="utf-8") == MOD_BASE
    assert (Path(worktree) / "setup-ran").exists()

    assert f"Task-ID: {replay_task}" in started["brief"]
    assert f"Trial-ID: {trial.id}" in started["brief"]
    assert "tests/test_g.py" in started["brief"]
    assert "add g" in started["brief"]


def test_correct_implementation_grades_pass(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert (graded.status, graded.outcome, graded.failure_cause) == ("graded", "pass", None)
    assert graded.features["actual"]["src_lines"] > 0
    assert graded.outcome_detail["flags"] == {}
    assert [record["exit"] for record in graded.outcome_detail["oracle"]] == [0]
    assert graded.outcome_detail["base_sha"] == started["base_sha"]
    assert graded.outcome_detail["run"] is None


def test_edited_test_file_fails_as_gaming(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    _write(started["worktree"], "tests/test_g.py", TEST_G_EDITED)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert graded.outcome == "fail"
    assert graded.outcome_detail["flags"] == {"tests_modified": ["tests/test_g.py"]}
    assert (graded.failure_cause, graded.failure_cause_source) == ("gaming", "mechanical")


def test_stub_in_source_fails_with_stub_flag(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_G_STUB)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert graded.outcome == "fail"
    assert [stub["path"] for stub in graded.outcome_detail["flags"]["stubs"]] == ["pkg/mod.py"]
    assert "tests_modified" not in graded.outcome_detail["flags"]
    assert graded.failure_cause == "gaming"


def test_no_change_fails_without_flags_or_cause(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    app.trial_runner.start(human, ws, trial.id)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert graded.outcome == "fail"
    assert graded.outcome_detail["flags"] == {}
    assert graded.failure_cause is None
    assert graded.outcome_detail["oracle"][0]["exit"] != 0


def test_failing_gate_fails_the_trial(app, human, ws, project, replay_task):
    app.experiments.create(
        human,
        ws,
        "gated",
        "Gated",
        None,
        protocol(gates=[[sys.executable, "-c", "raise SystemExit(1)"]]),
    )
    gated = app.experiments.freeze(human, ws, "gated")
    trial = app.experiments.add_trial(human, ws, gated.slug, replay_task, "haiku", "staircase")
    started = app.trial_runner.start(human, ws, trial.id)
    assert "raise SystemExit(1)" in started["brief"]
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert graded.outcome == "fail"
    assert [record["exit"] for record in graded.outcome_detail["gates"]] == [1]
    assert graded.outcome_detail["flags"] == {}
    assert graded.failure_cause is None


def test_run_on_other_model_marks_trial_invalid_and_keeps_outcome(
    app, human, ws, project, replay_task, running
):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    run_id = _record_run(app, trial.id, OLDER)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert (graded.status, graded.outcome) == ("invalid", "pass")
    assert (graded.failure_cause, graded.failure_cause_source) == ("wrong_model", "mechanical")
    assert graded.subagent_run_id == run_id
    assert graded.attempt_id == "attempt-7"


def test_run_on_pinned_model_keeps_trial_graded(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    run_id = _record_run(app, trial.id, PINNED)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert (graded.status, graded.outcome, graded.failure_cause) == ("graded", "pass", None)
    assert graded.outcome_detail["run"] == {
        "subagent_run_id": run_id,
        "attempt_id": "attempt-7",
        "model": PINNED,
    }
    assert graded.outcome_detail["escape_check"] == "no_transcript"


def test_failing_setup_fails_start_and_leaves_trial_planned(app, human, ws, project, replay_task):
    app.experiments.create(
        human,
        ws,
        "bad-setup",
        "Bad setup",
        None,
        protocol(setup=[[sys.executable, "-c", "raise SystemExit(3)"]]),
    )
    frozen = app.experiments.freeze(human, ws, "bad-setup")
    trial = app.experiments.add_trial(human, ws, frozen.slug, replay_task, "haiku", "staircase")
    with pytest.raises(ValidationFailed, match="setup"):
        app.trial_runner.start(human, ws, trial.id)
    assert app.experiments.trial(human, ws, trial.id).status == "planned"


def test_start_requires_planned_and_grade_requires_worktree(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    app.trial_runner.start(human, ws, trial.id)
    with pytest.raises(ConflictError):
        app.trial_runner.start(human, ws, trial.id)

    bare = _trial(app, human, ws, replay_task)
    app.experiments.start_trial(human, ws, bare.id)
    with pytest.raises(ValidationFailed, match="worktree"):
        app.trial_runner.grade(human, ws, bare.id)

    planned = _trial(app, human, ws, replay_task)
    with pytest.raises(ConflictError):
        app.trial_runner.grade(human, ws, planned.id)


def test_cli_start_and_grade(home, monkeypatch, tmp_path, app, human, ws, project, replay_task, running):
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

    trial = _trial(app, human, ws, replay_task)
    started = invoke("experiment", "trial", "start", trial.id)
    assert started["trial"]["status"] == "running"
    assert started["branch"] == f"wg/trial/{trial.id[:8]}"
    assert f"Trial-ID: {trial.id}" in started["brief"]

    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    graded = invoke("experiment", "trial", "grade", trial.id, "--timeout", "600")
    assert (graded["status"], graded["outcome"]) == ("graded", "pass")


def _transcript(path: Path, *calls: tuple[str, str, dict[str, Any]]) -> str:
    """A subagent transcript whose one assistant message makes the given (tool_use id, name, input) calls."""
    content = [
        {"type": "tool_use", "id": tool_id, "name": name, "input": data} for tool_id, name, data in calls
    ]
    record = {
        "type": "assistant",
        "timestamp": "2026-10-14T08:55:00.000Z",
        "message": {
            "id": "msg_1",
            "model": PINNED,
            "content": content,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    }
    path.write_text(json.dumps(record) + "\n", encoding="utf-8")
    return str(path)


def test_reading_the_source_checkout_fails_as_escaped_clone(
    app, human, ws, project, replay_task, running, tmp_path
):
    repo, _shas = project
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    transcript = _transcript(
        tmp_path / "agent-escape.jsonl",
        ("t1", "Read", {"file_path": str(repo.path / "pkg" / "mod.py")}),
    )
    _record_run(app, trial.id, PINNED, transcript)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert (graded.outcome, graded.failure_cause, graded.failure_cause_source) == (
        "fail",
        "gaming",
        "mechanical",
    )
    assert graded.outcome_detail["escape_check"] == "checked"
    flags = graded.outcome_detail["flags"]
    assert list(flags) == ["escaped_clone"]
    assert [(flag["tool"], flag["tool_use_id"]) for flag in flags["escaped_clone"]] == [("Read", "t1")]


def test_transcript_confined_to_the_clone_is_not_flagged(
    app, human, ws, project, replay_task, running, tmp_path
):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)
    transcript = _transcript(
        tmp_path / "agent-clone.jsonl",
        ("t1", "Read", {"file_path": str(Path(started["worktree"], "pkg", "mod.py"))}),
        ("t2", "Bash", {"command": f"cd {started['worktree']} && python -m pytest -q"}),
    )
    _record_run(app, trial.id, PINNED, transcript)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert (graded.status, graded.outcome) == ("graded", "pass")
    assert graded.outcome_detail["flags"] == {}
    assert graded.outcome_detail["escape_check"] == "checked"


def test_no_linked_run_reports_no_run(app, human, ws, project, replay_task, running):
    trial = _trial(app, human, ws, replay_task)
    started = app.trial_runner.start(human, ws, trial.id)
    _write(started["worktree"], "pkg/mod.py", MOD_WITH_G)

    graded = app.trial_runner.grade(human, ws, trial.id)
    assert graded.outcome == "pass"
    assert graded.outcome_detail["escape_check"] == "no_run"
    assert graded.outcome_detail["flags"] == {}


def test_isolated_clone_has_no_alternates_and_no_solution_objects(app, human, ws, project, replay_task):
    _repo, shas = project
    experiment = app.experiments.create(
        human, ws, "isolated-exp", "Isolated", None, protocol(clone="isolated")
    )
    frozen = app.experiments.freeze(human, ws, experiment.slug)
    trial = app.experiments.add_trial(human, ws, frozen.slug, replay_task, "haiku", "staircase")
    started = app.trial_runner.start(human, ws, trial.id)

    worktree = Path(started["worktree"])
    assert started["trial"].status == "running"
    assert not Path(worktree, ".git", "objects", "info", "alternates").exists()
    probe = subprocess.run(
        ["git", "-C", str(worktree), "cat-file", "-e", shas["eligible"]],
        capture_output=True,
        check=False,
    )
    assert probe.returncode != 0


def test_unknown_clone_mode_is_rejected_by_the_policy(app, human, ws):
    with pytest.raises(ValidationFailed, match="clone"):
        app.experiments.create(human, ws, "bad-clone", "Bad clone", None, protocol(clone="bogus"))
