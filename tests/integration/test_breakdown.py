from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.errors import ValidationFailed
from workgraph.core.models.schemas import ClaimRequest, TaskCreate, TaskFilter
from workgraph.core.services import breakdown

SOURCE = "docs/plan.md"

PLAN = """\
# Plan

## Breakdown

**V1. Parse the input**

* Depends on: V4. Tier: mid.

Read the file.

**V2. Report**

* Depends on: V1, WG-SEED. Q1 answered.

Write the report.

**V3. Notes**

Standalone, no dependencies.

**GV-01 to GV-02 Golden vectors** (each: a script)
Deps: V3.

| Id | Name |
| --- | --- |
| GV-01 | lifecycle |
| GV-02 | signatures |

**V4. Schema** Deps: none. Tier: top

Define the schema.

## Appendix

**V9. Outside the section** Deps: V1.
"""


def _accepted(app, human, ws, title):
    key = app.tasks.create(human, ws, TaskCreate(title=title, status="ready")).key
    app.tasks.claim(human, ws, key, ClaimRequest())
    app.tasks.complete(human, ws, key, summary="done")
    app.verification.accept_manually(human, ws, key, reason="reviewed")
    return key


def _run(app, human, ws, markdown=PLAN, *, section="Breakdown", parent=None, tags=(), dry_run=False):
    return breakdown.import_breakdown(
        app.tasks,
        human,
        ws,
        markdown,
        section=section,
        source=SOURCE,
        parent=parent,
        tags=tags,
        dry_run=dry_run,
    )


def _all_tasks(app, human, ws):
    out = []
    offset = 0
    while True:
        page = app.tasks.list(human, ws, TaskFilter(include_archived=True, limit=500, offset=offset))
        out.extend(page.items)
        if page.next_offset is None:
            return out
        offset = page.next_offset


def _by_id(result):
    return {row["id"]: row for row in result["tasks"]}


def test_import_creates_tasks_with_metadata_tags_and_status(app, human, ws):
    seed = _accepted(app, human, ws, "Seed")
    parent = app.tasks.create(human, ws, TaskCreate(title="Epic")).key
    markdown = PLAN.replace("WG-SEED", seed)

    result = _run(app, human, ws, markdown, parent=parent, tags=["imported"])

    rows = _by_id(result)
    assert set(rows) == {"V1", "V2", "V3", "GV-01", "GV-02", "V4"}
    assert result["counts"] == {"created": 6, "updated": 0, "unchanged": 0, "ready": 2, "backlog": 4}
    # V4 has no deps and V3 has none; V1 waits for V4; GV rows wait for V3; V2 has an unresolved item.
    assert rows["V4"]["status"] == "ready"
    assert rows["V3"]["status"] == "ready"
    assert rows["V1"]["status"] == "backlog"
    assert rows["GV-01"]["status"] == "backlog"
    assert rows["V2"]["status"] == "backlog"

    v1 = app.tasks.get(human, ws, rows["V1"]["key"])
    assert v1.metadata == {"breakdown_id": "V1", "breakdown_doc": f"{SOURCE}#Breakdown"}
    assert v1.title == "Parse the input"
    assert sorted(v1.tags) == ["imported", "tier-mid"]
    assert v1.depends_on == [app.tasks.get(human, ws, rows["V4"]["key"]).id]
    assert v1.parent_id == app.tasks.get(human, ws, parent).id

    v4 = app.tasks.get(human, ws, rows["V4"]["key"])
    assert sorted(v4.tags) == ["imported", "tier-top"]
    assert v4.priority == "medium"

    gv = app.tasks.get(human, ws, rows["GV-02"]["key"])
    assert gv.title == "Golden vectors: signatures"
    assert gv.metadata["breakdown_id"] == "GV-02"
    assert gv.depends_on == [app.tasks.get(human, ws, rows["V3"]["key"]).id]

    # The dependency on a key outside the section is resolved; the free text is reported.
    v2 = app.tasks.get(human, ws, rows["V2"]["key"])
    assert sorted(v2.depends_on) == sorted(
        [app.tasks.get(human, ws, rows["V1"]["key"]).id, app.tasks.get(human, ws, seed).id]
    )
    assert result["unresolved"] == [{"id": "V2", "dependency": "Q1 answered", "reason": "not a task id"}]
    assert "Q1 answered" in v2.description

    # The appendix is outside the section and was not imported.
    assert "V9" not in result["map"]


def test_dependency_on_accepted_key_makes_task_ready(app, human, ws):
    seed = _accepted(app, human, ws, "Seed")
    markdown = "## S\n\n**A1 Needs the seed** Deps: " + seed + ".\n"

    result = _run(app, human, ws, markdown, section="S")

    assert _by_id(result)["A1"]["status"] == "ready"
    assert result["unresolved"] == []


def test_rerun_is_idempotent(app, human, ws):
    _run(app, human, ws)
    before = {t.key for t in _all_tasks(app, human, ws)}

    again = _run(app, human, ws)

    assert {t.key for t in _all_tasks(app, human, ws)} == before
    assert again["counts"]["created"] == 0
    assert again["counts"]["updated"] == 0
    assert again["counts"]["unchanged"] == 6


def test_rerun_updates_changed_text_and_adds_new_dependency(app, human, ws):
    first = _run(app, human, ws)
    keys = {row["id"]: row["key"] for row in first["tasks"]}

    edited = PLAN.replace("Write the report.", "Write the full report.").replace(
        "Standalone, no dependencies.", "Standalone.\n\n* Depends on: V4."
    )
    result = _run(app, human, ws, edited)

    rows = _by_id(result)
    assert rows["V2"]["action"] == "updated"
    assert rows["V3"]["action"] == "updated"
    assert rows["V1"]["action"] == "unchanged"
    assert rows["V2"]["key"] == keys["V2"]
    assert len(_all_tasks(app, human, ws)) == len(first["tasks"])
    assert "Write the full report." in app.tasks.get(human, ws, keys["V2"]).description
    v3 = app.tasks.get(human, ws, keys["V3"])
    assert v3.depends_on == [app.tasks.get(human, ws, keys["V4"]).id]


def test_rerun_promotes_backlog_task_once_dependency_is_accepted(app, human, ws):
    markdown = "## S\n\n**A1 Waits** Deps: B1.\n\n**B1 First**\n"
    first = _run(app, human, ws, markdown, section="S")
    assert _by_id(first)["A1"]["status"] == "backlog"
    b1 = app.tasks.get(human, ws, _by_id(first)["B1"]["key"])
    app.tasks.claim(human, ws, b1.key, ClaimRequest())
    app.tasks.complete(human, ws, b1.key, summary="done")
    app.verification.accept_manually(human, ws, b1.key, reason="reviewed")

    second = _run(app, human, ws, markdown, section="S")

    assert _by_id(second)["A1"]["status"] == "ready"
    assert app.tasks.get(human, ws, _by_id(first)["A1"]["key"]).status == "ready"


def test_dry_run_creates_nothing_and_reports_the_plan(app, human, ws):
    result = _run(app, human, ws, dry_run=True)

    assert result["dry_run"] is True
    assert all(row["key"] is None for row in result["tasks"])
    assert result["counts"]["created"] == 6
    assert _all_tasks(app, human, ws) == []


def test_unknown_section_and_empty_section_are_refused(app, human, ws):
    with pytest.raises(ValidationFailed, match="no heading"):
        _run(app, human, ws, section="Nope")
    with pytest.raises(ValidationFailed, match="holds no tasks"):
        _run(app, human, ws, "## Empty\n\nNo tasks here.\n", section="Empty")
    assert _all_tasks(app, human, ws) == []


def test_refused_section_changes_nothing(app, human, ws):
    bad = "## S\n\n**V1. Good**\n\n**GV-01 to GV-02 Ranged**\n\nno table here\n"
    with pytest.raises(ValidationFailed):
        _run(app, human, ws, bad, section="S")
    assert _all_tasks(app, human, ws) == []


@pytest.fixture
def cli_run(home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    runner = CliRunner()

    def invoke(*args: str, expect: int = 0, json_out: bool = True) -> str:
        STATE.json = json_out
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        return result.output

    return invoke


def test_cli_import_breakdown_json_dry_run_and_rerun(cli_run, tmp_path: Path):
    cli_run("init", "--workspace", "acme", "--prefix", "AC")
    parent = json.loads(cli_run("task", "create", "Epic"))["key"]
    plan = tmp_path / "plan.md"
    plan.write_text(PLAN, encoding="utf-8")
    args = ["task", "import-breakdown", str(plan), "--section", "Breakdown", "--parent", parent]

    dry = json.loads(cli_run(*args, "--dry-run"))
    assert dry["dry_run"] is True
    assert dry["counts"]["created"] == 6
    assert json.loads(cli_run("task", "list"))["total"] == 1

    made = json.loads(cli_run(*args, "--tag", "imported"))
    assert made["counts"]["created"] == 6
    assert made["map"]["V1"].startswith("AC-")
    # The seed key in the fixture is not a real task here, so it is reported along with the free text.
    assert sorted(item["dependency"] for item in made["unresolved"]) == ["Q1 answered", "WG-SEED"]

    again = json.loads(cli_run(*args))
    assert again["counts"]["created"] == 0
    assert again["counts"]["unchanged"] == 6
    assert json.loads(cli_run("task", "list"))["total"] == 7


def test_cli_import_breakdown_table_output(cli_run, tmp_path: Path):
    cli_run("init", "--workspace", "acme", "--prefix", "AC")
    parent = json.loads(cli_run("task", "create", "Epic"))["key"]
    plan = tmp_path / "plan.md"
    plan.write_text(PLAN, encoding="utf-8")
    text = cli_run(
        "task",
        "import-breakdown",
        str(plan),
        "--section",
        "Breakdown",
        "--parent",
        parent,
        "--dry-run",
        json_out=False,
    )
    assert "dry run" in text
    assert "nothing changed" in text
