"""Tracker-supplied task keys, task_links and the external_refs migration."""

from __future__ import annotations

import pytest
from sqlalchemy import text

from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, PermissionDenied, ValidationFailed
from workgraph.core.models.schemas import TaskCreate
from workgraph.integrations.trackers.types import SYNC_ACTOR
from workgraph.persistence import migrate
from workgraph.persistence.db import Database
from workgraph.persistence.orm import TaskLink


@pytest.fixture
def sync() -> RequestContext:
    return RequestContext.system("tracker-sync")


def test_sync_actor_creates_task_with_tracker_key(app, sync, ws):
    assert sync.actor == SYNC_ACTOR
    out = app.tasks.create(sync, ws, TaskCreate(key="ABC-7", title="From Jira"))
    assert out.key == "ABC-7"
    # The task still consumes a workspace number, so the next local task continues the counter.
    local = app.tasks.create(RequestContext.human("tester"), ws, TaskCreate(title="Local"))
    assert local.number == out.number + 1
    assert local.key == f"WG-{local.number}"
    assert app.tasks.get(sync, ws, "ABC-7").title == "From Jira"


def test_human_and_agent_cannot_set_a_tracker_key(app, ws, agent_ctx):
    with pytest.raises(PermissionDenied):
        app.tasks.create(RequestContext.human("tester"), ws, TaskCreate(key="ABC-8", title="x"))
    with pytest.raises(PermissionDenied):
        app.tasks.create(agent_ctx, ws, TaskCreate(key="ABC-8", title="x"))


@pytest.mark.parametrize("key", ["abc-1", "ABC1", "ABC-", "ABC-x", "1ABC-1", "ABCDEFGHIJK-1"])
def test_tracker_key_must_match_the_format(app, sync, ws, key):
    with pytest.raises(ValidationFailed):
        app.tasks.create(sync, ws, TaskCreate(key=key, title="x"))


def test_tracker_key_prefix_cannot_be_the_workspace_prefix(app, sync, ws):
    with pytest.raises(ValidationFailed):
        app.tasks.create(sync, ws, TaskCreate(key="WG-99", title="x"))


def test_duplicate_tracker_key_conflicts(app, sync, ws):
    app.tasks.create(sync, ws, TaskCreate(key="ABC-7", title="first"))
    with pytest.raises(ConflictError):
        app.tasks.create(sync, ws, TaskCreate(key="ABC-7", title="second"))


def test_rejected_tracker_key_does_not_consume_a_number(app, sync, ws):
    human = RequestContext.human("tester")
    before = app.tasks.create(human, ws, TaskCreate(title="before"))
    with pytest.raises(ValidationFailed):
        app.tasks.create(sync, ws, TaskCreate(key="WG-1", title="bad"))
    after = app.tasks.create(human, ws, TaskCreate(title="after"))
    assert after.number == before.number + 1


def test_task_out_lists_links_ordered_by_kind_then_url(app, sync, ws):
    out = app.tasks.create(sync, ws, TaskCreate(key="ABC-9", title="With links"))
    with app.db.session(write=True) as session:
        for kind, url in [
            ("pull_request", "https://github.com/acme/svc-b/pull/2"),
            ("pull_request", "https://github.com/acme/svc-a/pull/1"),
            ("external_ref", "https://tracker.example/1"),
        ]:
            session.add(
                TaskLink(
                    task_id=out.id,
                    kind=kind,
                    provider="github" if kind == "pull_request" else "other",
                    url=url,
                    normalized_url=url.removeprefix("https://"),
                    repo_full_name="acme/svc",
                    sources=["custom_field"],
                    title="PR",
                )
            )
    fetched = app.tasks.get(sync, ws, "ABC-9")
    assert [(link.kind, link.url) for link in fetched.links] == [
        ("external_ref", "https://tracker.example/1"),
        ("pull_request", "https://github.com/acme/svc-a/pull/1"),
        ("pull_request", "https://github.com/acme/svc-b/pull/2"),
    ]
    assert fetched.links[1].sources == ["custom_field"]
    assert fetched.links[1].repository_id is None


def test_migration_0009_moves_external_refs_into_task_links(tmp_path):
    db = Database(tmp_path / "links.db")
    migrate.upgrade(db, "0008")
    with db.engine.begin() as conn:
        conn.exec_driver_sql(
            "INSERT INTO workspaces (id, slug, name, description, key_prefix, next_task_number, settings, created_at, "
            "updated_at) VALUES ('w', 's', 'n', '', 'WG', 2, '{}', '2026-01-01', '2026-01-01')"
        )
        conn.exec_driver_sql(
            "INSERT INTO tasks (id, workspace_id, number, key, title, description, description_hash, "
            "acceptance_criteria, status, priority, auto_execute, external_refs, metadata, created_by, version, "
            "created_at, updated_at) VALUES ('t1', 'w', 1, 'WG-1', 'Linked', '', 'h', '[]', 'backlog', 'medium', 0, "
            '\'[{"system":"jira","id":"ABC-1","url":"https://acme.atlassian.net/browse/ABC-1"},'
            "{\"system\":\"jira\",\"id\":\"ABC-2\"}]', '{}', 'test', 1, '2026-01-01', '2026-01-01')"
        )
    migrate.upgrade(db)
    assert migrate.current_revision(db) == "0009" == migrate.head_revision()
    with db.engine.connect() as conn:
        rows = conn.execute(
            text(
                "SELECT task_id, kind, provider, url, normalized_url, repo_full_name, sources, title, created_by "
                "FROM task_links ORDER BY normalized_url"
            )
        ).all()
        columns = {row[1] for row in conn.exec_driver_sql("PRAGMA table_info(tasks)")}
    assert "external_refs" not in columns
    assert [tuple(row) for row in rows] == [
        (
            "t1",
            "external_ref",
            "jira",
            "https://acme.atlassian.net/browse/ABC-1",
            "jira:ABC-1",
            "",
            '["migrated"]',
            "ABC-1",
            "system:migration",
        ),
        ("t1", "external_ref", "jira", "", "jira:ABC-2", "", '["migrated"]', "ABC-2", "system:migration"),
    ]

    migrate.downgrade(db, "0008")
    with db.engine.connect() as conn:
        columns = {row[1] for row in conn.exec_driver_sql("PRAGMA table_info(tasks)")}
        assert "external_refs" in columns
        assert conn.exec_driver_sql("SELECT COUNT(*) FROM task_links").scalar() == 0
        assert conn.exec_driver_sql("SELECT external_refs FROM tasks").scalar() == "[]"
