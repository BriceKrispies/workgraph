from __future__ import annotations

from datetime import timedelta

import pytest

from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError
from workgraph.integrations.trackers.sync import TrackerSyncService
from workgraph.integrations.trackers.types import SyncReport
from workgraph.jobs.handlers import tracker_sync
from workgraph.jobs.maintenance import run_maintenance
from workgraph.jobs.trackers import enqueue_tracker_sync
from workgraph.jobs.worker import JobContext


def _connection(*, workspace: str = "work") -> JiraTrackerConfig:
    return JiraTrackerConfig(
        base_url="https://acme.atlassian.net",
        workspace=workspace,
        projects=["ABC"],
        token_env="WG_TEST_JIRA_TOKEN",
        poll_interval_seconds=30,
        full_sync_interval_seconds=300,
    )


def _configure(app, **connections: JiraTrackerConfig) -> None:
    config = app.config
    config.trackers.clear()
    config.trackers.update(connections)
    app.config_store.save_global(config)


def _queued(app, kind: str = "tracker_sync") -> list:
    from sqlalchemy import select

    from workgraph.persistence.orm import Job

    with app.db.session() as session:
        return [
            (j.payload or {}, j.status)
            for j in session.scalars(select(Job).where(Job.kind == kind).order_by(Job.created_at))
        ]


def test_tracker_sync_handler_calls_sync_with_connection_and_mode(app, monkeypatch):
    _configure(app, **{"work-jira": _connection()})
    seen: list[tuple[str, str, bool]] = []

    def fake_sync(self, ctx, connection, *, full=False):
        self.connection(connection)
        seen.append((ctx.actor, connection, full))
        return SyncReport(connection=connection, full=full, fetched=3, updated=1, cursor=utcnow())

    monkeypatch.setattr(TrackerSyncService, "sync", fake_sync)
    job_id = enqueue_tracker_sync(app, RequestContext.human("tester"), "work-jira", full=True)
    claimed = app.queue.claim("w1")
    assert claimed is not None and claimed.id == job_id
    result = tracker_sync(app, JobContext(job=claimed, worker_id="w1"))
    assert seen == [("system:tracker-sync", "work-jira", True)]
    assert result["connection"] == "work-jira" and result["full"] is True
    assert result["fetched"] == 3 and result["updated"] == 1
    assert result["cursor"] is not None


def test_enqueue_is_a_no_op_while_a_sync_is_active(app):
    _configure(app, **{"work-jira": _connection()})
    ctx = RequestContext.human("tester")
    first = enqueue_tracker_sync(app, ctx, "work-jira", full=False)
    assert enqueue_tracker_sync(app, ctx, "work-jira", full=False) == first
    running = app.queue.claim("w1")
    assert running is not None and running.id == first
    assert enqueue_tracker_sync(app, ctx, "work-jira", full=False) == first  # still running
    # A full sync is a different job from an incremental one.
    full = enqueue_tracker_sync(app, ctx, "work-jira", full=True)
    assert full != first
    app.queue.complete(first, "w1")
    later = enqueue_tracker_sync(app, ctx, "work-jira", full=False)
    assert later not in (first, full)  # finished syncs never block a new one


def test_enqueue_uses_one_resource_key_per_connection(app):
    _configure(app, **{"work-jira": _connection(), "other": _connection(workspace="other")})
    ctx = RequestContext.human("tester")
    enqueue_tracker_sync(app, ctx, "work-jira", full=False)
    enqueue_tracker_sync(app, ctx, "other", full=False)
    first = app.queue.claim("w1")
    assert first is not None and first.payload["connection"] == "work-jira"
    assert app.queue.claim("w2") is not None  # the other connection is not held back
    assert app.queue.claim("w3") is None


def test_enqueue_rejects_unknown_connection(app):
    _configure(app, **{"work-jira": _connection()})
    with pytest.raises(NotFoundError):
        enqueue_tracker_sync(app, RequestContext.human("tester"), "nope", full=False)
    assert _queued(app) == []


def test_maintenance_enqueues_each_sync_once_per_interval(app, monkeypatch):
    _configure(app, **{"work-jira": _connection()})
    base = utcnow()
    now = {"at": base}
    monkeypatch.setattr("workgraph.jobs.maintenance.utcnow", lambda: now["at"])

    first = run_maintenance(app, "maint-1")
    assert first["tracker_syncs"] == {"incremental": 1, "full": 1, "failed": 0}
    assert sorted(p["full"] for p, _ in _queued(app)) == [False, True]

    # Nothing is due again until the poll interval has elapsed.
    again = run_maintenance(app, "maint-1")
    assert again["tracker_syncs"] == {"incremental": 0, "full": 0, "failed": 0}
    assert len(_queued(app)) == 2

    # Poll interval elapsed, full interval not: one more incremental, and it is a no-op while the
    # earlier incremental is still queued.
    now["at"] = base + timedelta(seconds=31)
    poll = run_maintenance(app, "maint-1")
    assert poll["tracker_syncs"] == {"incremental": 1, "full": 0, "failed": 0}
    assert len(_queued(app)) == 2

    # Once the queued sync has finished, the next poll queues a new one.
    claimed = app.queue.claim("w1")
    app.queue.complete(claimed.id, "w1")
    now["at"] = base + timedelta(seconds=62)
    poll = run_maintenance(app, "maint-1")
    assert poll["tracker_syncs"]["incremental"] == 1
    assert len(_queued(app)) == 3

    now["at"] = base + timedelta(seconds=301)
    full = run_maintenance(app, "maint-1")
    assert full["tracker_syncs"]["full"] == 1


def test_failing_connection_does_not_break_maintenance(app, monkeypatch):
    _configure(
        app,
        broken=_connection(workspace="broken"),
        **{"work-jira": _connection()},
    )
    from workgraph.jobs import trackers as tracker_jobs

    real_enqueue = tracker_jobs.enqueue_tracker_sync

    def flaky_enqueue(app_, ctx, connection, *, full):
        if connection == "broken":
            raise RuntimeError("boom")
        return real_enqueue(app_, ctx, connection, full=full)

    monkeypatch.setattr(tracker_jobs, "enqueue_tracker_sync", flaky_enqueue)
    report = run_maintenance(app, "maint-2")
    assert report["leader"] is True
    assert report["tracker_syncs"] == {"incremental": 1, "full": 1, "failed": 2}
    assert "replayed_hooks" in report  # the steps after the failing one still ran
    assert [p["connection"] for p, _ in _queued(app)] == ["work-jira", "work-jira"]
