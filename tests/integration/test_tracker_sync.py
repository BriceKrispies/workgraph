"""Tracker sync: Jira issues mirrored into tasks through an injected fake client and mapper."""

from __future__ import annotations

from collections.abc import Callable, Generator, Iterator
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from typing import Any

import pytest
from sqlalchemy import select

import workgraph.integrations.trackers.sync as sync_module
from tests.fixtures.fake_tracker import BASE_URL, FakeTrackerClient, fake_mapper, issue
from workgraph.app import Workgraph
from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import RepositoryRegister
from workgraph.integrations.trackers.sync import TrackerSyncService
from workgraph.integrations.trackers.types import SYNC_ACTOR
from workgraph.persistence.orm import (
    Event,
    ExternalItem,
    Project,
    SystemCursor,
    Task,
    TaskDependency,
    TaskLink,
    TaskProject,
    TaskRepository,
)

CONNECTION = "work-jira"
CURSOR = f"tracker:{CONNECTION}:updated"
T0 = datetime(2026, 10, 1, 10, 0, tzinfo=UTC)


def at(minutes: int) -> str:
    return (T0 + timedelta(minutes=minutes)).isoformat()


@pytest.fixture
def tracker_app(app: Workgraph, ws: str) -> Workgraph:
    config = app.config
    config.trackers[CONNECTION] = JiraTrackerConfig(
        base_url=BASE_URL,
        workspace=ws,
        projects=["ABC"],
        token_env="WORKGRAPH_JIRA_TOKEN",
        missing_grace_seconds=3600,
    )
    app.config_store.save_global(config)
    return app


def service(app: Workgraph, client: FakeTrackerClient) -> TrackerSyncService:
    return TrackerSyncService(app, client_factory=lambda _config: client, mapper=fake_mapper)


def find_task(app: Workgraph, key: str) -> dict[str, Any] | None:
    with app.db.session() as session:
        task = session.scalar(select(Task).where(Task.key == key))
        if task is None:
            return None
        return {
            "id": task.id,
            "title": task.title,
            "description": task.description,
            "priority": task.priority,
            "status": task.status,
            "assignee": task.assignee,
            "parent_id": task.parent_id,
            "archived": task.archived_at is not None,
            "created_by": task.created_by,
        }


def task_row(app: Workgraph, key: str) -> dict[str, Any]:
    row = find_task(app, key)
    assert row is not None, key
    return row


def events(app: Workgraph, event_type: str) -> list[dict[str, Any]]:
    with app.db.session() as session:
        rows = session.scalars(select(Event).where(Event.event_type == event_type).order_by(Event.seq)).all()
        return [{**row.data, "_actor": row.actor} for row in rows]


def task_id(app: Workgraph, key: str) -> str:
    return task_row(app, key)["id"]


def cursor_value(app: Workgraph, name: str = CURSOR) -> int | None:
    with app.db.session() as session:
        row = session.get(SystemCursor, name)
        return None if row is None else row.value


def register_repo(
    app: Workgraph, human: RequestContext, make_repo: Callable[..., Any], name: str, remote: str
) -> str:
    repo = make_repo(name)
    repo.git("remote", "add", "origin", remote)
    out = app.repositories.register(human, "main", RepositoryRegister(path=str(repo.path)))
    return out.id


def test_creates_tasks_projects_and_external_items(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", title="Ship it", priority="high", assignee="ana")])
    report = service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert (report.fetched, report.created, report.updated, report.unchanged) == (1, 1, 0, 0)
    row = task_row(tracker_app, "ABC-1")
    assert row is not None
    assert (row["title"], row["priority"], row["assignee"], row["created_by"]) == (
        "Ship it",
        "high",
        "ana",
        SYNC_ACTOR,
    )
    with tracker_app.db.session() as session:
        slugs = session.scalars(
            select(Project.slug)
            .join(TaskProject, TaskProject.project_id == Project.id)
            .where(TaskProject.task_id == row["id"])
        ).all()
        item = session.scalar(select(ExternalItem).where(ExternalItem.external_key == "ABC-1"))
        assert item is not None and item.connection == CONNECTION
        assert item.tracker_status_category == "new"
        assert item.snapshot["key"] == "ABC-1"
    assert list(slugs) == ["abc"]
    created = events(tracker_app, "task.synced")
    assert created == [
        {
            "connection": CONNECTION,
            "external_key": "ABC-1",
            "created": True,
            "changed_fields": [],
            "_actor": SYNC_ACTOR,
        }
    ]


def test_update_writes_only_changed_fields(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", title="Old", description="keep", updated=at(0))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)

    client.issues["id-ABC-1"] = issue("ABC-1", title="New", description="keep", updated=at(30))
    report = svc.sync(RequestContext.system("cli"), CONNECTION)

    assert (report.updated, report.unchanged) == (1, 0)
    assert task_row(tracker_app, "ABC-1")["title"] == "New"
    synced = events(tracker_app, "task.synced")
    assert synced[-1]["changed_fields"] == ["title"]
    update = events(tracker_app, "task.updated")[-1]
    assert update["changes"] == {"title": ["Old", "New"]}


def test_unchanged_issue_emits_no_event(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", updated=at(0))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)
    before = len(events(tracker_app, "task.synced"))

    report = svc.sync(RequestContext.system("cli"), CONNECTION)

    assert (report.unchanged, report.updated, report.created) == (1, 0, 0)
    assert len(events(tracker_app, "task.synced")) == before


def test_child_is_applied_after_its_parent_and_blocker(tracker_app: Workgraph) -> None:
    # The child is listed first (oldest update), so the page must reorder it behind its parent and blocker.
    client = FakeTrackerClient(
        [
            issue("ABC-2", parent_key="ABC-1", blocked_by_keys=["ABC-3"], updated=at(0)),
            issue("ABC-1", updated=at(5)),
            issue("ABC-3", updated=at(6)),
        ]
    )
    report = service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert report.created == 3
    assert report.errors == []
    assert task_row(tracker_app, "ABC-2")["parent_id"] == task_id(tracker_app, "ABC-1")
    with tracker_app.db.session() as session:
        dep = session.scalar(
            select(TaskDependency).where(TaskDependency.task_id == task_id(tracker_app, "ABC-2"))
        )
        assert dep is not None
        assert dep.depends_on_id == task_id(tracker_app, "ABC-3")
        assert dep.created_by == SYNC_ACTOR


def test_missing_parent_is_fetched_before_the_write(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-6", updated=at(0))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)

    # ABC-8 is older than the cursor minus overlap, so only a direct fetch can bring it in.
    client.issues["id-ABC-8"] = issue("ABC-8", title="Epic", updated=at(-600))
    client.issues["id-ABC-7"] = issue("ABC-7", parent_key="ABC-8", updated=at(10))
    report = svc.sync(RequestContext.system("cli"), CONNECTION)

    assert client.method_calls("get_issue") == [("ABC-8",)]
    assert report.created == 2
    assert task_row(tracker_app, "ABC-8")["title"] == "Epic"
    assert task_row(tracker_app, "ABC-7")["parent_id"] == task_id(tracker_app, "ABC-8")


def test_parent_outside_configured_projects_is_not_fetched(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", parent_key="OPS-9", blocked_by_keys=["OPS-4"], updated=at(0))])
    report = service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert client.method_calls("get_issue") == []
    assert report.created == 1
    assert task_row(tracker_app, "ABC-1")["parent_id"] is None


def test_removed_blocker_drops_the_sync_dependency(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient(
        [issue("ABC-1", updated=at(0)), issue("ABC-2", blocked_by_keys=["ABC-1"], updated=at(1))]
    )
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)
    blocker_id = task_id(tracker_app, "ABC-1")
    with tracker_app.db.session() as session:
        assert (
            session.scalar(select(TaskDependency).where(TaskDependency.depends_on_id == blocker_id))
            is not None
        )

    client.issues["id-ABC-2"] = issue("ABC-2", blocked_by_keys=[], updated=at(20))
    report = svc.sync(RequestContext.system("cli"), CONNECTION)

    assert report.updated == 1
    with tracker_app.db.session() as session:
        assert (
            session.scalar(select(TaskDependency).where(TaskDependency.depends_on_id == blocker_id)) is None
        )
    assert events(tracker_app, "task.dependency_removed")[-1]["depends_on"] == blocker_id


def test_pull_requests_link_resolved_and_unresolved_repositories(
    tracker_app: Workgraph, human: RequestContext, make_repo: Callable[..., Any]
) -> None:
    repo_id = register_repo(tracker_app, human, make_repo, "svc-a", "https://github.com/acme/svc-a.git")
    pr_a = "https://github.com/acme/svc-a/pull/42"
    pr_other = "https://github.com/acme/other/pull/7"
    client = FakeTrackerClient(
        [issue("ABC-1", pull_requests=[pr_a, pr_other], updated=at(0))],
        dev_panels={"id-ABC-1": [{"url": pr_a, "title": "Add the thing"}]},
    )
    svc = service(tracker_app, client)
    report = svc.sync(RequestContext.system("cli"), CONNECTION)

    assert report.unresolved_pull_requests == ["github.com/acme/other"]
    with tracker_app.db.session() as session:
        links = {
            link.normalized_url: link
            for link in session.scalars(
                select(TaskLink).where(TaskLink.task_id == task_id(tracker_app, "ABC-1"))
            ).all()
        }
        assert set(links) == {"github.com/acme/svc-a/pull/42", "github.com/acme/other/pull/7"}
        assert links["github.com/acme/svc-a/pull/42"].repository_id == repo_id
        assert sorted(links["github.com/acme/svc-a/pull/42"].sources) == ["custom_field", "dev_panel"]
        assert links["github.com/acme/other/pull/7"].repository_id is None
        assert links["github.com/acme/svc-a/pull/42"].created_by == SYNC_ACTOR
        repo_links = session.scalars(
            select(TaskRepository.repository_id).where(
                TaskRepository.task_id == task_id(tracker_app, "ABC-1")
            )
        ).all()
        assert list(repo_links) == [repo_id]
    assert svc.status(RequestContext.system("cli"), CONNECTION)["unresolved_pull_request_repos"] == [
        "acme/other"
    ]

    # The pull request disappears from the issue: its sync-created link goes, the other stays.
    client.issues["id-ABC-1"] = issue("ABC-1", pull_requests=[pr_other], updated=at(30))
    client.dev_panels.clear()
    svc.sync(RequestContext.system("cli"), CONNECTION)
    with tracker_app.db.session() as session:
        urls = session.scalars(
            select(TaskLink.normalized_url).where(TaskLink.task_id == task_id(tracker_app, "ABC-1"))
        ).all()
    assert list(urls) == ["github.com/acme/other/pull/7"]


def test_done_issues_are_accepted_or_cancelled(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient(
        [
            issue("ABC-1", status_name="Done", status_category="done", resolution="Done", updated=at(0)),
            issue("ABC-2", status_name="Done", status_category="done", resolution="Won't Do", updated=at(1)),
            issue("ABC-3", status_name="In Progress", status_category="indeterminate", updated=at(2)),
        ]
    )
    service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert task_row(tracker_app, "ABC-1")["status"] == "accepted"
    assert task_row(tracker_app, "ABC-2")["status"] == "cancelled"
    assert task_row(tracker_app, "ABC-3")["status"] == "backlog"


def test_cursor_advances_and_the_next_search_overlaps(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", updated=at(0)), issue("ABC-2", updated=at(15))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)

    assert cursor_value(tracker_app) == int((T0 + timedelta(minutes=15)).timestamp() * 1000)
    svc.sync(RequestContext.system("cli"), CONNECTION)
    (first_since,), (second_since,) = [args[1:] for args in client.method_calls("search_updated")][:2]
    assert first_since is None
    assert second_since == T0 + timedelta(minutes=15) - timedelta(seconds=120)


def test_full_reconcile_marks_then_archives_after_grace(
    tracker_app: Workgraph, monkeypatch: pytest.MonkeyPatch
) -> None:
    clock = [T0]
    monkeypatch.setattr(sync_module, "utcnow", lambda: clock[0])
    client = FakeTrackerClient([issue("ABC-1", updated=at(0)), issue("ABC-2", updated=at(0))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION, full=True)

    del client.issues["id-ABC-2"]
    clock[0] = T0 + timedelta(minutes=5)
    report = svc.sync(RequestContext.system("cli"), CONNECTION, full=True)
    assert (report.missing, report.archived) == (1, 0)
    assert task_row(tracker_app, "ABC-2")["archived"] is False

    clock[0] = T0 + timedelta(hours=2)
    report = svc.sync(RequestContext.system("cli"), CONNECTION, full=True)
    assert (report.missing, report.archived) == (1, 1)
    assert task_row(tracker_app, "ABC-2")["archived"] is True
    assert task_row(tracker_app, "ABC-1")["archived"] is False

    # The issue comes back: the mark is cleared and the task is restored.
    client.issues["id-ABC-2"] = issue("ABC-2", updated=at(0))
    report = svc.sync(RequestContext.system("cli"), CONNECTION, full=True)
    assert report.missing == 0
    assert task_row(tracker_app, "ABC-2")["archived"] is False
    assert svc.status(RequestContext.system("cli"), CONNECTION)["missing"] == 0


def test_dev_panel_failure_is_recorded_and_sync_continues(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient(
        [issue("ABC-1", updated=at(0)), issue("ABC-2", updated=at(1))], fail_dev_panel=True
    )
    report = service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert report.created == 2
    assert len(report.errors) == 2
    assert all("dev panel" in error for error in report.errors)
    assert find_task(tracker_app, "ABC-1") is not None


class _FailingMidSearch(FakeTrackerClient):
    def search_updated(self, projects: Any, since: Any) -> Iterator[dict[str, Any]]:
        items = iter(super().search_updated(projects, since))
        yield next(items)
        yield next(items)
        raise ConnectionError("Jira unreachable")


def test_search_error_closes_client_and_keeps_applied_pages(
    tracker_app: Workgraph, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(sync_module, "PAGE_SIZE", 2)
    client = _FailingMidSearch(
        [issue("ABC-1", updated=at(0)), issue("ABC-2", updated=at(1)), issue("ABC-3", updated=at(2))]
    )
    with pytest.raises(ConnectionError):
        service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION)

    assert client.closed is True
    assert find_task(tracker_app, "ABC-1") is not None
    assert find_task(tracker_app, "ABC-2") is not None
    assert find_task(tracker_app, "ABC-3") is None
    assert cursor_value(tracker_app) == int((T0 + timedelta(minutes=1)).timestamp() * 1000)


def test_no_write_transaction_is_open_during_client_calls(
    tracker_app: Workgraph, monkeypatch: pytest.MonkeyPatch
) -> None:
    open_writes = {"count": 0}
    real_session = tracker_app.db.session

    @contextmanager
    def counting_session(*, write: bool = False) -> Generator[Any]:
        if write:
            open_writes["count"] += 1
        try:
            with real_session(write=write) as session:
                yield session
        finally:
            if write:
                open_writes["count"] -= 1

    monkeypatch.setattr(tracker_app.db, "session", counting_session)
    seen: list[tuple[str, int]] = []
    client = FakeTrackerClient(
        [issue("ABC-1", parent_key="ABC-0", updated=at(0)), issue("ABC-2", updated=at(1))],
        dev_panels={"id-ABC-2": []},
    )
    client.on_call = lambda method, _args: seen.append((method, open_writes["count"]))
    service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION, full=True)

    methods = {method for method, _ in seen}
    assert {"search_updated", "dev_pull_requests", "get_issue", "list_keys", "close"} <= methods
    assert all(count == 0 for _, count in seen)


def test_moved_issue_changes_the_task_key(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", title="Moved", updated=at(0))])
    svc = service(tracker_app, client)
    svc.sync(RequestContext.system("cli"), CONNECTION)
    task = task_id(tracker_app, "ABC-1")

    client.issues["id-ABC-1"] = issue("ABC-9", id="id-ABC-1", title="Moved", updated=at(10))
    svc.sync(RequestContext.system("cli"), CONNECTION)

    assert find_task(tracker_app, "ABC-1") is None
    assert task_id(tracker_app, "ABC-9") == task
    with tracker_app.db.session() as session:
        item = session.scalar(select(ExternalItem).where(ExternalItem.external_id == "id-ABC-1"))
        assert item is not None and item.external_key == "ABC-9"
    assert events(tracker_app, "task.updated")[-1]["changes"] == {"key": ["ABC-1", "ABC-9"]}


def test_status_reports_cursor_counts_and_unresolved_repositories(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", updated=at(0)), issue("ABC-2", updated=at(3))])
    svc = service(tracker_app, client)
    before = svc.status(RequestContext.system("cli"), CONNECTION)
    assert before["cursor"] is None
    assert before["last_sync"] is None
    assert before["tracked_tasks"] == 0

    svc.sync(RequestContext.system("cli"), CONNECTION)
    status = svc.status(RequestContext.system("cli"), CONNECTION)

    assert status["connection"] == CONNECTION
    assert status["workspace"] == "main"
    assert status["projects"] == ["ABC"]
    assert status["cursor"] == (T0 + timedelta(minutes=3)).isoformat()
    assert status["last_sync"] is not None
    assert status["last_full_sync"] is None
    assert status["tracked_tasks"] == 2
    assert status["missing"] == 0
    assert status["unresolved_pull_request_repos"] == []


def test_status_rejects_unknown_connection(tracker_app: Workgraph) -> None:
    from workgraph.core.errors import NotFoundError

    with pytest.raises(NotFoundError):
        service(tracker_app, FakeTrackerClient()).status(RequestContext.system("cli"), "nope")


def test_sync_report_is_serialisable(tracker_app: Workgraph) -> None:
    client = FakeTrackerClient([issue("ABC-1", updated=at(0))])
    report = service(tracker_app, client).sync(RequestContext.system("cli"), CONNECTION, full=True)
    data = report.as_dict()
    assert data["full"] is True
    assert data["created"] == 1
    assert data["cursor"] == (T0).isoformat()
