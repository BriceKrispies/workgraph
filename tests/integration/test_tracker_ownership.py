"""Tracked tasks: tracker-owned fields, tracker-driven status, and no verification auto-accept."""

from __future__ import annotations

import sys
from typing import Literal

import pytest
from sqlalchemy import select

from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.context import RequestContext
from workgraph.core.errors import PermissionDenied
from workgraph.core.models.schemas import (
    ClaimRequest,
    CommentCreate,
    RepositoryRegister,
    TaskCreate,
    TaskOut,
    TaskUpdate,
)
from workgraph.integrations.trackers.types import SYNC_ACTOR
from workgraph.persistence.orm import Event, ExternalItem, Task

CONNECTION = "work-jira"


@pytest.fixture
def sync_ctx() -> RequestContext:
    return RequestContext.system("tracker-sync")


@pytest.fixture
def jira(app) -> JiraTrackerConfig:
    """A `[trackers.work-jira]` connection in the test app's global config."""
    connection = JiraTrackerConfig(
        base_url="https://example.atlassian.net",
        workspace="main",
        projects=["ABC"],
        token_env="WG_TEST_JIRA_TOKEN",
    )
    config = app.config
    config.trackers[CONNECTION] = connection
    app.config_store.save_global(config)
    return connection


def make_tracked(
    app, human, ws, *, status: Literal["backlog", "ready"] = "ready", title: str = "Mirrored work"
) -> TaskOut:
    """A local task that mirrors a Jira issue: an ExternalItem row names it."""
    task = app.tasks.create(human, ws, TaskCreate(title=title, status=status))
    with app.db.session(write=True) as session:
        session.add(
            ExternalItem(
                workspace_id=task.workspace_id,
                task_id=task.id,
                connection=CONNECTION,
                external_id=f"id-{task.key}",
                external_key=f"ABC-{task.number}",
                tracker_status="To Do",
                tracker_status_category="new",
            )
        )
    return app.tasks.get(human, ws, task.key)


def sync_issue(app, task, *, category: str, resolution: str | None = None, status_name: str = "Done") -> None:
    """What the sync does for one issue: store its status, then let the task follow it."""
    with app.db.session(write=True) as session:
        item = session.scalar(select(ExternalItem).where(ExternalItem.task_id == task.id))
        item.tracker_status = status_name
        item.tracker_status_category = category
        item.tracker_resolution = resolution
        app.tasks.apply_tracker_status(
            session, RequestContext.system("tracker-sync"), session.get(Task, task.id), item
        )


def events_of(app, task, event_type: str) -> list[Event]:
    with app.db.session() as session:
        return list(
            session.scalars(
                select(Event)
                .where(Event.entity_id == task.id, Event.event_type == event_type)
                .order_by(Event.seq)
            )
        )


OWNED_EDITS = [
    pytest.param({"title": "Renamed locally"}, id="title"),
    pytest.param({"description": "Local notes"}, id="description"),
    pytest.param({"acceptance_criteria": ["Local criteria"]}, id="acceptance_criteria"),
    pytest.param({"priority": "critical"}, id="priority"),
    pytest.param({"assignee": "someone"}, id="assignee"),
]


@pytest.mark.parametrize("edit", OWNED_EDITS)
def test_human_edits_to_tracker_owned_fields_are_rejected(app, human, ws, edit):
    task = make_tracked(app, human, ws)
    with pytest.raises(PermissionDenied, match="the tracker owns"):
        app.tasks.update(human, ws, task.key, TaskUpdate(**edit))
    assert app.tasks.get(human, ws, task.key).title == "Mirrored work"


@pytest.mark.parametrize("edit", OWNED_EDITS)
def test_agent_edits_to_tracker_owned_fields_are_rejected(app, agent_ctx, human, ws, edit):
    task = make_tracked(app, human, ws)
    with pytest.raises(PermissionDenied, match="the tracker owns"):
        app.tasks.update(agent_ctx, ws, task.key, TaskUpdate(**edit))


def test_parent_change_is_rejected_both_ways(app, human, ws, sync_ctx):
    parent = app.tasks.create(human, ws, TaskCreate(title="Epic"))
    task = make_tracked(app, human, ws)
    with pytest.raises(PermissionDenied):
        app.tasks.update(human, ws, task.key, TaskUpdate(parent=parent.key))
    app.tasks.update(sync_ctx, ws, task.key, TaskUpdate(parent=parent.key))
    with pytest.raises(PermissionDenied):
        app.tasks.update(human, ws, task.key, TaskUpdate(clear_parent=True))


def test_unchanged_owned_values_are_not_rejected(app, human, ws):
    """The guard only fires on a value that actually changes."""
    task = make_tracked(app, human, ws)
    out = app.tasks.update(human, ws, task.key, TaskUpdate(title="Mirrored work", add_tags=["triage"]))
    assert out.tags == ["triage"]


def test_sync_actor_may_edit_tracker_owned_fields(app, human, ws, sync_ctx):
    task = make_tracked(app, human, ws)
    out = app.tasks.update(sync_ctx, ws, task.key, TaskUpdate(title="From Jira", priority="high"))
    assert out.title == "From Jira"
    assert out.priority == "high"


def test_agent_keeps_claim_comment_and_tag_rights(app, agent_ctx, human, ws):
    task = make_tracked(app, human, ws)
    claimed = app.tasks.claim(agent_ctx, ws, task.key, ClaimRequest(agent="test-agent"))
    assert claimed.status == "claimed"
    app.tasks.release(agent_ctx, ws, task.key, reason="done for now")
    app.tasks.add_comment(agent_ctx, ws, task.key, CommentCreate(body="looked into it"))
    tagged = app.tasks.update(agent_ctx, ws, task.key, TaskUpdate(add_tags=["backend"]))
    assert tagged.tags == ["backend"]


def test_human_cannot_cancel_a_tracked_task(app, human, ws):
    task = make_tracked(app, human, ws, status="backlog")
    with pytest.raises(PermissionDenied, match="the tracker owns status"):
        app.tasks.update(human, ws, task.key, TaskUpdate(status="cancelled"))
    assert app.tasks.get(human, ws, task.key).status == "backlog"


def test_human_cannot_reopen_a_closed_tracked_task(app, human, ws, sync_ctx):
    task = make_tracked(app, human, ws, status="backlog")
    app.tasks.update(sync_ctx, ws, task.key, TaskUpdate(status="cancelled"))
    with pytest.raises(PermissionDenied):
        app.tasks.update(human, ws, task.key, TaskUpdate(status="backlog"))


def test_archive_of_a_tracked_task_is_rejected_for_humans(app, human, ws, sync_ctx):
    task = make_tracked(app, human, ws)
    with pytest.raises(PermissionDenied):
        app.tasks.archive(human, ws, task.key)
    assert app.tasks.archive(sync_ctx, ws, task.key).archived_at is not None


def test_local_dependencies_can_be_added_and_removed(app, human, ws):
    blocker = app.tasks.create(human, ws, TaskCreate(title="Local blocker"))
    task = make_tracked(app, human, ws)
    app.tasks.add_dependency(human, ws, task.key, blocker.key)
    assert app.tasks.remove_dependency(human, ws, task.key, blocker.key).depends_on == []


def test_sync_created_dependency_cannot_be_removed_by_humans(app, human, ws, sync_ctx):
    blocker = app.tasks.create(human, ws, TaskCreate(title="Upstream issue"))
    dependent = app.tasks.create(sync_ctx, ws, TaskCreate(title="Mirrored", depends_on=[blocker.key]))
    with pytest.raises(PermissionDenied, match="comes from the tracker"):
        app.tasks.remove_dependency(human, ws, dependent.key, blocker.key)
    assert app.tasks.get(human, ws, dependent.key).depends_on == [blocker.id]


def test_done_issue_accepts_the_task_and_releases_dependents(app, human, ws, jira):
    task = make_tracked(app, human, ws, status="ready")
    dependent = app.tasks.create(
        human, ws, TaskCreate(title="Waits for upstream", status="ready", depends_on=[task.key])
    )
    assert app.tasks.get(human, ws, dependent.key).status == "blocked"

    sync_issue(app, task, category="done", resolution="Done")

    assert app.tasks.get(human, ws, task.key).status == "accepted"
    (acceptance,) = app.verification.acceptances(human, ws, task.key)
    assert acceptance.mode == "external"
    assert acceptance.reason == f"external:{CONNECTION}"
    assert acceptance.verification_run_id is None
    assert acceptance.actor == SYNC_ACTOR
    assert app.tasks.get(human, ws, dependent.key).status == "ready"
    assert len(events_of(app, task, "task.accepted")) == 1


def test_wont_do_cancels_the_task(app, human, ws, jira):
    task = make_tracked(app, human, ws, status="ready")
    sync_issue(app, task, category="done", resolution="Won't Do")
    assert app.tasks.get(human, ws, task.key).status == "cancelled"
    assert app.verification.acceptances(human, ws, task.key) == []


def test_leaving_done_reopens_the_task_to_backlog(app, human, ws, jira):
    task = make_tracked(app, human, ws, status="ready")
    sync_issue(app, task, category="done", resolution="Done")
    assert app.tasks.get(human, ws, task.key).status == "accepted"

    sync_issue(app, task, category="indeterminate", status_name="In Progress")

    assert app.tasks.get(human, ws, task.key).status == "backlog"
    (reopened,) = events_of(app, task, "task.reopened")
    assert reopened.data["from"] == "accepted"
    assert reopened.data["to"] == "backlog"


def test_indeterminate_issue_leaves_the_status_alone(app, human, ws, jira):
    task = make_tracked(app, human, ws, status="ready")
    sync_issue(app, task, category="indeterminate", status_name="In Progress")
    assert app.tasks.get(human, ws, task.key).status == "ready"


def test_manual_acceptance_of_a_tracked_task_is_rejected(app, human, ws, jira):
    task = make_tracked(app, human, ws, status="ready")
    with pytest.raises(PermissionDenied, match="the tracker closes it"):
        app.verification.accept_manually(human, ws, task.key, reason="looks fine")
    with pytest.raises(PermissionDenied, match="the tracker closes it"):
        app.verification.accept(human, ws, task.key)


def test_passing_verification_does_not_accept_a_tracked_task(app, human, ws, make_repo, jira):
    repo = make_repo("mirror")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    task = app.tasks.create(
        human, ws, TaskCreate(title="Checked work", repositories=["mirror"], status="ready")
    )
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="mirror",
        name="ci",
        checks=[{"name": "unit tests", "command": [sys.executable, "-c", "print('1 passed')"]}],
    )
    with app.db.session(write=True) as session:
        session.add(
            ExternalItem(
                workspace_id=task.workspace_id,
                task_id=task.id,
                connection=CONNECTION,
                external_id="id-checked",
                external_key="ABC-99",
            )
        )
        session.get(Task, task.id).status = "awaiting_verification"

    run = app.verification.run(human, ws, task.key)

    assert run.required_passed
    assert app.tasks.get(human, ws, task.key).status == "awaiting_review"
    assert app.verification.acceptances(human, ws, task.key) == []
