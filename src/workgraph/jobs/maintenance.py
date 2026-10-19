"""Periodic maintenance performed by exactly one elected worker at a time."""

from __future__ import annotations

from datetime import timedelta
from typing import TYPE_CHECKING, Any

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ResourceBusyError
from workgraph.core.logging import get_logger
from workgraph.persistence.orm import SystemCursor

if TYPE_CHECKING:
    from workgraph.app import Workgraph

log = get_logger("maintenance")
RETENTION_INTERVAL = timedelta(hours=24)


def _elect(app: Workgraph, worker_id: str, lease_seconds: int) -> bool:
    from workgraph.integrations.git.worktrees import acquire_lease

    try:
        with app.db.session(write=True) as session:
            acquire_lease(
                session, "system:maintenance", worker_id, purpose="maintenance", ttl_seconds=lease_seconds
            )
        return True
    except ResourceBusyError:
        return False


def run_maintenance(app: Workgraph, worker_id: str, *, lease_seconds: int = 30) -> dict[str, Any]:
    if not _elect(app, worker_id, lease_seconds):
        return {"leader": False}
    ctx = RequestContext.system("maintenance")
    report: dict[str, Any] = {"leader": True}
    report["recovered_jobs"] = app.queue.recover_expired()
    report["lost_workers"] = app.queue.mark_lost_workers()
    report["released_claims"] = app.tasks.release_expired_claims(ctx)
    report["automation"] = app.automation.process_events(ctx)
    report["scheduled_rules"] = app.automation.run_scheduled(ctx)
    config = app.config
    if config.execution.enabled and config.execution.auto_schedule:
        report["scheduled_runs"] = app.scheduler.tick(ctx)
    report["replayed_hooks"] = app.hooks.replay_spool(limit=200)
    report["tracker_syncs"] = _enqueue_tracker_syncs(app, ctx)
    if _due(app, "retention", RETENTION_INTERVAL):
        from workgraph.jobs.retention import compact

        report["retention"] = compact(app)
    return report


def _enqueue_tracker_syncs(app: Workgraph, ctx: RequestContext) -> dict[str, int]:
    """Queue each configured tracker's incremental and full syncs once their intervals elapse.

    A failing connection is logged and skipped so that the other maintenance steps still run.
    """
    from workgraph.jobs.trackers import enqueue_tracker_sync

    counts = {"incremental": 0, "full": 0, "failed": 0}
    for name, connection in app.config.trackers.items():
        for full, mode, interval in (
            (False, "poll", timedelta(seconds=connection.poll_interval_seconds)),
            (True, "full", timedelta(seconds=connection.full_sync_interval_seconds)),
        ):
            try:
                if not _due(app, f"tracker:{name}:{mode}", interval):
                    continue
                enqueue_tracker_sync(app, ctx, name, full=full)
            except Exception:
                counts["failed"] += 1
                log.exception("tracker sync enqueue failed", extra={"connection": name, "mode": mode})
                continue
            counts["full" if full else "incremental"] += 1
    return counts


def _due(app: Workgraph, name: str, interval: timedelta) -> bool:
    now = utcnow()
    stamp = int(now.timestamp())
    with app.db.session(write=True) as session:
        cursor = session.get(SystemCursor, f"maintenance:{name}")
        if cursor is None:
            session.add(SystemCursor(name=f"maintenance:{name}", value=stamp))
            return True
        if stamp - cursor.value >= interval.total_seconds():
            cursor.value = stamp
            return True
        return False
