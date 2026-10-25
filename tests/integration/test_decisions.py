"""Structured decisions: service, REST API, MCP tools, CLI and triage."""

from __future__ import annotations

import json

import pytest
from mcp import Client
from starlette.testclient import TestClient
from typer.testing import CliRunner

from workgraph.api.app import create_app
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.models.schemas import TaskCreate
from workgraph.core.services import triage
from workgraph.mcp.server import build_server
from workgraph.persistence import migrate
from workgraph.persistence.db import Database

OPTIONS = [
    {"label": "Use SQLite", "detail": "Zero setup", "recommended": True},
    {"label": "Use Postgres", "detail": "Needs a server"},
]


def _task(app, human, ws, title="Pick a store", tags=None):
    return app.tasks.create(human, ws, TaskCreate(title=title, tags=tags or [])).key


def _tags(app, human, ws, key):
    return set(app.tasks.get(human, ws, key).tags)


def _comments(app, human, ws, key):
    return [c.body for c in app.tasks.detail(human, ws, key).comments]


# ------------------------------------------------------------------ service


def test_request_tags_task_and_fills_option_ids(app, human, ws):
    key = _task(app, human, ws)
    d = app.decisions.request(human, ws, key, "Which store?", OPTIONS)
    assert d.status == "open" and d.task_key == key and d.version == 1
    assert [o.id for o in d.options] == ["a", "b"]
    assert [o.recommended for o in d.options] == [True, False]
    assert "needs-decision" in _tags(app, human, ws, key)


def test_request_without_options_and_explicit_ids(app, human, ws):
    key = _task(app, human, ws)
    free = app.decisions.request(human, ws, key, "What should we call it?")
    assert free.options == []
    named = app.decisions.request(
        human, ws, key, "Which?", [{"id": "sqlite", "label": "SQLite"}, {"label": "Other"}]
    )
    assert [o.id for o in named.options] == ["sqlite", "b"]
    with pytest.raises(ValidationFailed):
        app.decisions.request(human, ws, key, "Dup?", [{"id": "x", "label": "1"}, {"id": "x", "label": "2"}])


def test_list_orders_open_by_priority_and_skips_closed_tasks(app, human, ws):
    low = app.tasks.create(human, ws, TaskCreate(title="Low", priority="low")).key
    high = app.tasks.create(human, ws, TaskCreate(title="High", priority="high")).key
    gone = _task(app, human, ws, "Gone")
    app.decisions.request(human, ws, low, "low?", OPTIONS)
    app.decisions.request(human, ws, high, "high?", OPTIONS)
    app.decisions.request(human, ws, gone, "gone?", OPTIONS)
    app.tasks.archive(human, ws, gone)
    open_items = app.decisions.list(human, ws)
    assert [d.task_key for d in open_items] == [high, low]
    assert open_items[0].task.title == "High" and "needs-decision" in open_items[0].task.tags
    assert len(app.decisions.list(human, ws, "all")) == 3


def test_choose_invalid_option_is_refused(app, human, ws):
    key = _task(app, human, ws)
    d = app.decisions.request(human, ws, key, "Which store?", OPTIONS)
    with pytest.raises(ValidationFailed):
        app.decisions.choose(human, ws, d.id, option_id="nope")
    with pytest.raises(ValidationFailed):
        app.decisions.choose(human, ws, d.id)
    with pytest.raises(ValidationFailed):
        app.decisions.choose(human, ws, d.id, option_id="a", other_text="both")
    with pytest.raises(NotFoundError):
        app.decisions.choose(human, ws, "missing", option_id="a")
    assert app.decisions.get(human, ws, d.id).status == "open"
    assert "needs-decision" in _tags(app, human, ws, key)


def test_choosing_last_open_decision_releases_task(app, human, ws):
    key = _task(app, human, ws, tags=["parked", "triage:needs-decision", "keep"])
    d = app.decisions.request(human, ws, key, "Which store?", OPTIONS)
    chosen = app.decisions.choose(human, ws, d.id, option_id="b", note="ops already runs one")
    assert chosen.status == "decided" and chosen.chosen_option_id == "b"
    assert chosen.decided_by == "human:tester" and chosen.decided_at is not None and chosen.version == 2
    assert _tags(app, human, ws, key) == {"keep"}
    body = _comments(app, human, ws, key)[-1]
    assert body.startswith("User decision ") and "Which store? -> Use Postgres" in body
    assert body.endswith("\n\nNote: ops already runs one")


def test_choosing_with_another_open_decision_keeps_tags(app, human, ws):
    key = _task(app, human, ws, tags=["parked"])
    first = app.decisions.request(human, ws, key, "First?", OPTIONS)
    second = app.decisions.request(human, ws, key, "Second?", OPTIONS)
    app.decisions.choose(human, ws, first.id, option_id="a")
    assert {"needs-decision", "parked"} <= _tags(app, human, ws, key)
    app.decisions.choose(human, ws, second.id, other_text="Neither: use files")
    assert _tags(app, human, ws, key) == set()
    assert _comments(app, human, ws, key)[-1].endswith("Second? -> Neither: use files")
    stored = app.decisions.get(human, ws, second.id)
    assert stored.chosen_option_id is None and stored.other_text == "Neither: use files"


def test_changing_an_answer(app, human, ws):
    key = _task(app, human, ws)
    d = app.decisions.request(human, ws, key, "Which store?", OPTIONS)
    app.decisions.choose(human, ws, d.id, option_id="a")
    again = app.decisions.choose(human, ws, d.id, other_text="Actually, neither")
    assert again.status == "decided" and again.chosen_option_id is None
    assert again.other_text == "Actually, neither" and again.version == 3
    assert len([c for c in _comments(app, human, ws, key) if c.startswith("User decision")]) == 2
    assert "needs-decision" not in _tags(app, human, ws, key)


def test_withdraw(app, human, ws):
    key = _task(app, human, ws)
    first = app.decisions.request(human, ws, key, "First?", OPTIONS)
    second = app.decisions.request(human, ws, key, "Second?", OPTIONS)
    app.decisions.withdraw(human, ws, first.id, "no longer relevant")
    assert "needs-decision" in _tags(app, human, ws, key)
    gone = app.decisions.withdraw(human, ws, second.id)
    assert gone.status == "withdrawn" and gone.chosen_option_id is None
    assert "needs-decision" not in _tags(app, human, ws, key)
    comments = _comments(app, human, ws, key)
    assert comments[0] == "Decision withdrawn: First?\n\nReason: no longer relevant"
    assert not any(c.startswith("User decision") for c in comments)
    with pytest.raises(ValidationFailed):
        app.decisions.withdraw(human, ws, first.id)
    with pytest.raises(ValidationFailed):
        app.decisions.choose(human, ws, first.id, option_id="a")
    assert app.decisions.list(human, ws) == []
    assert [d.status for d in app.decisions.for_task(human, ws, key)] == ["withdrawn", "withdrawn"]


def test_choosing_emits_a_task_event(app, human, ws):
    key = _task(app, human, ws)
    d = app.decisions.request(human, ws, key, "Which store?", OPTIONS)
    app.decisions.choose(human, ws, d.id, option_id="a")
    types = [e.event_type for e in app.history.task_history(human, ws, key)]
    assert "task.decision_requested" in types and "task.decision_chosen" in types


# ------------------------------------------------------------------ API


@pytest.fixture
def client(app, ws, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_ALLOWED_HOSTS", "testserver")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=False)
    with TestClient(asgi, base_url="http://testserver") as c:
        c.headers["Authorization"] = f"Bearer {app.api_token()}"
        yield c


def test_api_round_trip(app, human, ws, client):
    key = _task(app, human, ws)
    base = f"/api/v1/ws/{ws}"
    made = client.post(
        f"{base}/tasks/{key}/decisions",
        json={
            "question": "Which store?",
            "options": [{"label": "SQLite", "recommended": True}, {"label": "PG"}],
        },
    )
    assert made.status_code == 201
    body = made.json()
    assert body["task_key"] == key and body["status"] == "open"
    assert body["options"][0] == {"id": "a", "label": "SQLite", "detail": "", "recommended": True}
    assert set(body) >= {
        "id", "task_id", "question", "options", "status", "chosen_option_id", "other_text", "note",
        "decided_by", "decided_at", "created_by", "created_at", "updated_at", "version",
    }  # fmt: skip

    listing = client.get(f"{base}/decisions").json()["items"]
    assert [d["id"] for d in listing] == [body["id"]]
    assert listing[0]["task"]["key"] == key and "needs-decision" in listing[0]["task"]["tags"]
    assert set(listing[0]["task"]) == {"key", "title", "status", "priority", "tags", "description"}
    assert [d["id"] for d in client.get(f"{base}/tasks/{key}/decisions").json()["items"]] == [body["id"]]
    assert client.get(f"{base}/decisions?status=decided").json()["items"] == []

    for bad in ({"option_id": "zzz"}, {}, {"option_id": "a", "other_text": "x"}):
        refused = client.post(f"{base}/decisions/{body['id']}/choose", json=bad)
        assert refused.status_code == 422
        assert refused.json()["error"]["code"] == "validation_failed"
    chosen = client.post(f"{base}/decisions/{body['id']}/choose", json={"option_id": "b", "note": "ok"})
    assert chosen.status_code == 200
    assert chosen.json()["status"] == "decided" and chosen.json()["chosen_option_id"] == "b"
    assert chosen.json()["note"] == "ok"
    assert client.get(f"{base}/decisions").json()["items"] == []
    assert len(client.get(f"{base}/decisions?status=decided").json()["items"]) == 1
    assert "needs-decision" not in app.tasks.get(human, ws, key).tags


def test_api_withdraw_and_unknown(client, app, human, ws):
    key = _task(app, human, ws)
    base = f"/api/v1/ws/{ws}"
    made = client.post(f"{base}/tasks/{key}/decisions", json={"question": "Free text?"}).json()
    assert made["options"] == []
    out = client.post(f"{base}/decisions/{made['id']}/withdraw", json={"reason": "moot"})
    assert out.status_code == 200 and out.json()["status"] == "withdrawn"
    assert client.post(f"{base}/decisions/nope/withdraw", json={}).status_code == 404
    assert client.post(f"{base}/tasks/{key}/decisions", json={"question": ""}).status_code == 422


# ------------------------------------------------------------------ MCP


async def test_mcp_tools_request_and_read_but_not_choose(app, human, ws):
    key = _task(app, human, ws)
    server = build_server(app, agent_name="tester")
    async with Client(server) as client:
        names = {t.name for t in (await client.list_tools()).tools}
        assert {"task_request_decision", "decision_list", "task_decisions"} <= names
        assert not any("choose" in n for n in names if "decision" in n)
        made = await client.call_tool(
            "task_request_decision",
            {"task": key, "question": "Which store?", "options": [{"label": "SQLite", "recommended": True}]},
        )
        assert not made.is_error
        decision_id = made.structured_content["id"]
        listed = await client.call_tool("decision_list", {})
        assert [d["id"] for d in listed.structured_content["decisions"]] == [decision_id]
        open_for_task = await client.call_tool("task_decisions", {"task": key})
        assert open_for_task.structured_content["decisions"][0]["status"] == "open"
        app.decisions.choose(human, ws, decision_id, option_id="a")
        answered = await client.call_tool("task_decisions", {"task": key})
        assert answered.structured_content["decisions"][0]["chosen_option_id"] == "a"
        none_open = await client.call_tool("decision_list", {"status": "open"})
        assert none_open.structured_content["decisions"] == []


# ------------------------------------------------------------------ CLI


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


def test_cli_decision_flow(app, human, ws, cli_run):
    key = _task(app, human, ws)
    made = json.loads(
        cli_run(
            "decision",
            "request",
            key,
            "--question",
            "Which store?",
            "--option",
            "SQLite::zero setup",
            "--option",
            "Postgres",
            "--recommend",
            "1",
        )
    )
    assert [(o["id"], o["detail"], o["recommended"]) for o in made["options"]] == [
        ("a", "zero setup", True),
        ("b", "", False),
    ]
    listed = json.loads(cli_run("decision", "list"))
    assert [d["id"] for d in listed] == [made["id"]]
    shown = json.loads(cli_run("decision", "show", made["id"]))
    assert shown["task"]["key"] == key
    assert "Which store?" in cli_run("decision", "show", made["id"], json_out=False)
    cli_run("decision", "choose", made["id"], "--option", "zzz", expect=2)
    cli_run("decision", "choose", made["id"], expect=2)
    chosen = json.loads(cli_run("decision", "choose", made["id"], "--option", "a", "--note", "fine"))
    assert chosen["status"] == "decided" and chosen["note"] == "fine"
    other = json.loads(cli_run("decision", "choose", made["id"][:8], "--other", "files"))
    assert other["other_text"] == "files" and other["chosen_option_id"] is None
    assert json.loads(cli_run("decision", "list")) == []
    assert len(json.loads(cli_run("decision", "list", "--status", "decided"))) == 1
    cli_run("decision", "list", "--status", "bogus", expect=2)

    second = json.loads(cli_run("decision", "request", key, "--question", "Again?"))
    out = json.loads(cli_run("decision", "withdraw", second["id"], "--reason", "moot"))
    assert out["status"] == "withdrawn"
    cli_run("decision", "request", key, "--question", "Q", "--option", "x", "--recommend", "3", expect=2)


# ------------------------------------------------------------------ triage


def _apply(app, human, ws, entries):
    return triage.apply_verdicts(app.tasks, human, ws, entries, dry_run=False, source="triage batch TEST")


def test_triage_structured_decision_creates_a_decision(app, human, ws):
    key = _task(app, human, ws)
    entry = {
        "key": key,
        "verdict": "needs-decision",
        "summary": "Two ways to go.",
        "decision": {
            "question": "Which store?",
            "options": [{"label": "SQLite", "detail": "simple", "recommended": True}, "Postgres"],
        },
    }
    report = _apply(app, human, ws, [entry])
    assert report["errors"] == [] and report["applied"][0]["action"] == "commented"
    [decision] = app.decisions.for_task(human, ws, key)
    assert decision.question == "Which store?" and decision.status == "open"
    assert [(o.id, o.label, o.recommended) for o in decision.options] == [
        ("a", "SQLite", True),
        ("b", "Postgres", False),
    ]
    assert {"needs-decision", "triage:needs-decision"} <= _tags(app, human, ws, key)
    assert any("Decision: Which store?" in c for c in _comments(app, human, ws, key))
    # answering clears the triage tag too
    app.decisions.choose(human, ws, decision.id, option_id="a")
    assert _tags(app, human, ws, key) == set()


def test_triage_string_decision_still_works_and_bad_object_is_an_error(app, human, ws):
    key = _task(app, human, ws)
    other = _task(app, human, ws, "Other")
    report = _apply(
        app,
        human,
        ws,
        [
            {"key": key, "verdict": "needs-decision", "decision": "Pick one."},
            {"key": other, "verdict": "needs-decision", "decision": {"options": []}},
        ],
    )
    assert [e["key"] for e in report["errors"]] == [other]
    assert app.decisions.for_task(human, ws, key) == []
    assert "needs-decision" in _tags(app, human, ws, key)
    assert any("Decision: Pick one." in c for c in _comments(app, human, ws, key))


# ------------------------------------------------------------------ migration


def test_decisions_migration_up_and_down(tmp_path):
    db = Database(tmp_path / "d.db")
    migrate.upgrade(db, "0010")
    with db.engine.begin() as conn:
        conn.exec_driver_sql(
            "INSERT INTO workspaces (id, slug, name, description, key_prefix, next_task_number, settings, "
            "created_at, updated_at) VALUES ('w', 's', 'n', '', 'WG', 2, '{}', '2026-01-01', '2026-01-01')"
        )
        conn.exec_driver_sql(
            "INSERT INTO tasks (id, workspace_id, number, key, title, description, description_hash, "
            "acceptance_criteria, status, priority, auto_execute, metadata, created_by, version, "
            "created_at, updated_at) VALUES ('t1', 'w', 1, 'WG-1', 'T', '', 'h', '[]', 'backlog', 'medium', 0, "
            "'{}', 'test', 1, '2026-01-01', '2026-01-01')"
        )
    migrate.upgrade(db)
    with db.engine.begin() as conn:
        conn.exec_driver_sql(
            "INSERT INTO task_decisions (id, workspace_id, task_id, question, options, status, created_by, "
            "created_at, updated_at, version) VALUES ('d1', 'w', 't1', 'Q?', '[]', 'open', 'test', "
            "'2026-01-01', '2026-01-01', 1)"
        )
        conn.exec_driver_sql("DELETE FROM tasks WHERE id = 't1'")
        assert conn.exec_driver_sql("SELECT count(*) FROM task_decisions").scalar() == 0  # cascades
    migrate.downgrade(db, "0010")
    with db.engine.connect() as conn:
        assert (
            conn.exec_driver_sql("SELECT name FROM sqlite_master WHERE name = 'task_decisions'").all() == []
        )
    migrate.upgrade(db)
    assert migrate.is_up_to_date(db)
