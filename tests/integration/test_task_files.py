"""Tasks carry an optional structured ``files`` list: create, update, show, list filter, triage."""

from __future__ import annotations

import json

import pytest
from typer.testing import CliRunner

from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.models.schemas import TaskCreate, TaskFileIn, TaskFilter, TaskUpdate, parse_task_file
from workgraph.core.services import triage


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


def _json(cli_run, *args: str) -> dict:
    return json.loads(cli_run(*args))


def _files(detail) -> list[tuple[str, str | None]]:
    return [(f.path, f.anchor) for f in detail.files]


def test_parse_task_file_splits_anchor_and_keeps_drive_letters():
    assert parse_task_file("a/b.rs:fn_x") == TaskFileIn(path="a/b.rs", anchor="fn_x")
    assert parse_task_file("c.md") == TaskFileIn(path="c.md")
    assert parse_task_file("C:/src/x.rs:Foo::bar") == TaskFileIn(path="C:/src/x.rs", anchor="Foo::bar")
    assert parse_task_file("docs/a.md:") == TaskFileIn(path="docs/a.md")
    with pytest.raises(ValueError):
        parse_task_file(":fn_x")


def test_create_stores_files_in_order_and_show_lists_them(app, human, ws, cli_run):
    out = _json(cli_run, "task", "create", "Touch auth", "--file", "a/b.rs:fn_x", "--file", "c.md")
    assert out["files"] == [
        {"path": "a/b.rs", "anchor": "fn_x", "note": ""},
        {"path": "c.md", "anchor": None, "note": ""},
    ]
    key = out["key"]
    shown = _json(cli_run, "task", "show", key)
    assert shown["files"] == out["files"]
    text = cli_run("task", "show", key, json_out=False)
    assert "Files" in text and "a/b.rs:fn_x" in text and "c.md" in text


def test_bad_file_spec_refuses_the_whole_create(app, human, ws, cli_run):
    cli_run("task", "create", "Broken", "--file", ":fn_x", expect=1, json_out=False)
    assert app.tasks.list(human, ws, TaskFilter()).total == 0


def test_cli_update_replaces_appends_and_clears(app, human, ws, cli_run):
    key = _json(cli_run, "task", "create", "Edit me", "--file", "x.py")["key"]
    cli_run("task", "update", key, "--file", "a.py:f", "--file", "b.py")
    assert _files(app.tasks.detail(human, ws, key)) == [("a.py", "f"), ("b.py", None)]

    cli_run("task", "update", key, "--add-file", "b.py", "--add-file", "c.py:Thing")
    assert _files(app.tasks.detail(human, ws, key)) == [("a.py", "f"), ("b.py", None), ("c.py", "Thing")]

    cli_run("task", "update", key, "--clear-files")
    assert app.tasks.detail(human, ws, key).files == []

    cli_run("task", "update", key, "--file", "a.py", "--clear-files", expect=1, json_out=False)


def test_cli_list_filters_by_exact_path(app, human, ws, cli_run):
    hit = _json(cli_run, "task", "create", "Lib", "--file", "src/lib.rs")["key"]
    anchored = _json(cli_run, "task", "create", "Lib part", "--file", "src/lib.rs:Other")["key"]
    _json(cli_run, "task", "create", "Docs", "--file", "docs/x.md")
    listing = _json(cli_run, "task", "list", "--file", "src/lib.rs")
    assert listing["total"] == 2
    assert {t["key"] for t in listing["items"]} == {hit, anchored}


def test_service_update_add_and_replace(app, human, ws):
    created = app.tasks.create(
        human,
        ws,
        TaskCreate(title="Service path", files=[parse_task_file("a.py:f"), parse_task_file("a.py:f")]),
    )
    # a repeated (path, anchor) pair is kept once
    assert _files(app.tasks.detail(human, ws, created.key)) == [("a.py", "f")]

    app.tasks.update(
        human,
        ws,
        created.key,
        TaskUpdate(add_files=[TaskFileIn(path="b.md", note="check the retry text")]),
    )
    detail = app.tasks.detail(human, ws, created.key)
    assert _files(detail) == [("a.py", "f"), ("b.md", None)]
    assert detail.files[1].note == "check the retry text"

    app.tasks.update(human, ws, created.key, TaskUpdate(files=[TaskFileIn(path="z.py")]))
    assert _files(app.tasks.detail(human, ws, created.key)) == [("z.py", None)]
    assert [t.key for t in app.tasks.list(human, ws, TaskFilter(file="z.py")).items] == [created.key]


def _apply(app, human, ws, entries):
    return triage.apply_verdicts(app.tasks, human, ws, entries, dry_run=False, source="triage batch TEST")


def _open_task(app, human, ws, title):
    return app.tasks.create(human, ws, TaskCreate(title=title)).key


def _rewrite(**overrides):
    rewrite = {
        "title": "Rewritten task",
        "description": "Do the thing.",
        "files": ["src/a.py:run", "src/b.py"],
        "acceptance_criteria": ["it works"],
        "size": "one-file",
    }
    rewrite.update(overrides)
    return rewrite


def test_triage_apply_copies_rewrite_files_into_the_new_task(app, human, ws):
    key = _open_task(app, human, ws, "Split me")
    report = _apply(
        app,
        human,
        ws,
        [{"key": key, "verdict": "open", "summary": "Half is done.", "rewrites": [_rewrite()]}],
    )
    assert report["errors"] == []
    new_key = report["applied"][0]["new_keys"][0]
    detail = app.tasks.detail(human, ws, new_key)
    assert _files(detail) == [("src/a.py", "run"), ("src/b.py", None)]
    # the **Files:** line stays in the description, as before
    assert "**Files:** src/a.py:run, src/b.py" in detail.description


def test_triage_rewrite_without_files_creates_no_files(app, human, ws):
    key = _open_task(app, human, ws, "No files")
    report = _apply(
        app,
        human,
        ws,
        [{"key": key, "verdict": "open", "summary": "s", "rewrites": [_rewrite(files=[])]}],
    )
    new_key = report["applied"][0]["new_keys"][0]
    assert app.tasks.detail(human, ws, new_key).files == []


def test_triage_refuses_a_rewrite_file_that_is_not_a_path(app, human, ws):
    key = _open_task(app, human, ws, "Bad file")
    report = _apply(
        app,
        human,
        ws,
        [{"key": key, "verdict": "open", "summary": "s", "rewrites": [_rewrite(files=[":fn_x"])]}],
    )
    assert report["errors"][0]["error"] == "rewrite 0 has a file that is not a path: ':fn_x'"
    assert app.tasks.list(human, ws, TaskFilter(file="src/a.py")).total == 0
