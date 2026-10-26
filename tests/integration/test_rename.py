from __future__ import annotations

import json
from pathlib import Path

import pytest
from sqlalchemy import func, select

from tests.helpers import run_cli
from workgraph.core.errors import ConflictError
from workgraph.core.models.schemas import (
    ClaimRequest,
    ProjectCreate,
    ProjectUpdate,
    RepositoryRegister,
    RepositoryUpdate,
    TaskCreate,
    TaskFileIn,
)
from workgraph.core.services.rename import Term, rename_term
from workgraph.persistence.orm.ops import Job


@pytest.fixture
def world(app, human, ws, make_repo, tmp_path: Path):
    """A project `rho` over a repository `rho`, with an open task, a closed one and a check."""
    repo = make_repo("rho")
    app.repositories.register(
        human, ws, RepositoryRegister(path=str(repo.path), description="The Rho compiler")
    )
    app.projects.create(
        human, ws, ProjectCreate(slug="rho", name="Rho", description="Rho language", repositories=["rho"])
    )
    script = tmp_path / "rho_check.py"
    script.write_text("", encoding="utf-8")
    check = {
        "name": "tests",
        "kind": "test",
        "command": ["python", script.as_posix()],
        "env": {"RHO_TIER": "device", "PLAIN": "rhombus"},
        "repository": "rho",
    }
    app.verification.create_config(
        human, ws, scope_type="repository", scope="rho", name="rho-tests", checks=[check]
    )
    open_task = app.tasks.create(
        human,
        ws,
        TaskCreate(
            title="Teach the Rho driver a flag",
            description="`rho build` needs RHO_TIER; a rhombus is not the language.",
            acceptance_criteria=["rho check passes"],
            files=[TaskFileIn(path="apps/rho/src/main.rs", anchor="rho_main")],
            tags=["rho", "driver"],
            projects=["rho"],
        ),
    )
    closed = app.tasks.create(
        human, ws, TaskCreate(title="Old Rho fix", tags=["rho"], projects=["rho"], status="ready")
    )
    app.tasks.claim(human, ws, closed.key, ClaimRequest())
    app.tasks.complete(human, ws, closed.key, summary="done in rho")
    app.verification.accept_manually(human, ws, closed.key, reason="reviewed")
    return {"open": open_task.key, "closed": closed.key, "script": script}


def test_dry_run_reports_and_changes_nothing(app, human, ws, world):
    before = app.tasks.get(human, ws, world["open"])
    report = rename_term(app, human, ws, "rho", Term("Rho", "Nyn"), dry_run=True)
    assert report["errors"] == []
    assert report["project"] == {"slug": "rho", "new_slug": "nyn", "name": "Rho", "new_name": "Nyn"}
    assert [r["new_name"] for r in report["repositories"]] == ["nyn"]
    assert {t["key"] for t in report["tasks"]} == {world["open"], world["closed"]}
    assert report["closed_kept"] == 1
    after = app.tasks.get(human, ws, world["open"])
    assert after.title == before.title and after.version == before.version
    assert app.projects.get(human, ws, "rho").name == "Rho"


def test_open_tasks_project_repository_and_checks_are_renamed(app, human, ws, world):
    report = rename_term(app, human, ws, "rho", Term("Rho", "Nyn"))
    assert report["errors"] == []

    task = app.tasks.get(human, ws, world["open"])
    assert task.title == "Teach the Nyn driver a flag"
    assert task.description == "`nyn build` needs NYN_TIER; a rhombus is not the language."
    assert task.acceptance_criteria == ["nyn check passes"]
    assert [(f.path, f.anchor) for f in task.files] == [("apps/nyn/src/main.rs", "nyn_main")]
    assert sorted(task.tags) == ["driver", "nyn"]

    project = app.projects.get(human, ws, "nyn")
    assert (project.name, project.description) == ("Nyn", "Nyn language")
    repo = app.repositories.get(human, ws, "nyn")
    assert repo.description == "The Nyn compiler"

    (config,) = app.verification.list_configs(human, ws)
    (check,) = config.checks
    assert config.name == "rho-tests"
    assert check["repository"] == "nyn"
    assert check["env"] == {"NYN_TIER": "device", "PLAIN": "rhombus"}
    assert check["command"][1].endswith("nyn_check.py")
    (reported,) = report["verification_configs"]
    assert reported["missing_paths"] == [check["command"][1]]


def test_closed_tasks_keep_their_text_and_take_the_new_tag(app, human, ws, world):
    rename_term(app, human, ws, "rho", Term("Rho", "Nyn"))
    task = app.tasks.get(human, ws, world["closed"])
    assert task.title == "Old Rho fix"
    assert task.tags == ["nyn"]
    assert task.status == "accepted"


def test_each_change_is_an_event_and_queues_no_reclassification(app, human, ws, world):
    config = app.config
    config.llm.auto_classify = True
    app.config_store.save_global(config)
    rename_term(app, human, ws, "rho", Term("Rho", "Nyn"))
    history = app.history.task_history(human, ws, world["open"], event_types=["task.updated"])
    assert history[-1].data["reason"] == "rename Rho to Nyn"
    with app.db.session() as session:
        jobs = session.scalar(select(func.count()).select_from(Job).where(Job.kind == "classify_task"))
    assert jobs == 0


def test_running_it_twice_changes_nothing_the_second_time(app, human, ws, world):
    rename_term(app, human, ws, "rho", Term("Rho", "Nyn"))
    again = rename_term(app, human, ws, "nyn", Term("Rho", "Nyn"))
    assert (again["project"], again["repositories"], again["tasks"], again["verification_configs"]) == (
        None,
        [],
        [],
        [],
    )


def test_a_renamed_slug_or_repository_name_must_be_free(app, human, ws, world, make_repo):
    app.projects.create(human, ws, ProjectCreate(slug="nyn", name="Taken"))
    with pytest.raises(ConflictError):
        app.projects.update(human, ws, "rho", ProjectUpdate(slug="nyn"))
    other = make_repo("other")
    app.repositories.register(human, ws, RepositoryRegister(path=str(other.path), name="taken"))
    with pytest.raises(ConflictError):
        app.repositories.update(human, ws, "rho", RepositoryUpdate(name="taken"))
    report = rename_term(app, human, ws, "rho", Term("Rho", "Nyn"))
    assert any(e["target"] == "project rho" for e in report["errors"])


@pytest.fixture
def cli_run(home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))

    def invoke(*args: str, expect: int = 0) -> str:
        result = run_cli(["--json", *args])
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        return result.output

    return invoke


def test_cli_rename_term_dry_run_then_apply(cli_run):
    cli_run("init", "--workspace", "acme", "--prefix", "AC")
    cli_run("project", "create", "rho", "--name", "Rho")
    task = json.loads(cli_run("task", "create", "Port the Rho lexer", "--project", "rho", "--tag", "rho"))
    dry = json.loads(cli_run("project", "rename-term", "rho", "--from", "Rho", "--to", "Nyn", "--dry-run"))
    assert dry["dry_run"] is True and [t["key"] for t in dry["tasks"]] == [task["key"]]
    assert json.loads(cli_run("task", "show", task["key"]))["title"] == "Port the Rho lexer"
    cli_run("project", "rename-term", "rho", "--from", "Rho", "--to", "Nyn")
    shown = json.loads(cli_run("task", "show", task["key"]))
    assert shown["title"] == "Port the Nyn lexer" and shown["tags"] == ["nyn"]
