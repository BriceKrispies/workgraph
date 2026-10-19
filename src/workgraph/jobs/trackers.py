"""Enqueueing tracker syncs: at most one queued or running sync per connection and mode."""

from __future__ import annotations

from typing import TYPE_CHECKING

from sqlalchemy import select

from workgraph.core.clock import new_id
from workgraph.core.context import RequestContext
from workgraph.jobs.limits import tracker_key
from workgraph.jobs.queue import ACTIVE_STATUSES, enqueue
from workgraph.persistence.orm import Job

if TYPE_CHECKING:
    from workgraph.app import Workgraph

TRACKER_SYNC_KIND = "tracker_sync"


def enqueue_tracker_sync(app: Workgraph, ctx: RequestContext, connection: str, *, full: bool) -> str:
    """Queue a sync of ``connection`` and return its job id.

    A queued or running sync of the same connection and mode is returned instead of queued again.
    The check and the insert share one write transaction, so concurrent callers cannot both queue
    one. The idempotency key is unique per call: keyed on the connection alone, it would map every
    later sync onto the first, finished job.
    """
    app.trackers.connection(connection)  # raises NotFoundError for an unknown connection
    with app.db.session(write=True) as session:
        for active in session.scalars(
            select(Job).where(Job.kind == TRACKER_SYNC_KIND, Job.status.in_(ACTIVE_STATUSES))
        ):
            payload = active.payload or {}
            if payload.get("connection") == connection and bool(payload.get("full")) == full:
                return active.id
        mode = "full" if full else "incremental"
        job = enqueue(
            session,
            TRACKER_SYNC_KIND,
            {"connection": connection, "full": full},
            resource_keys=[tracker_key(connection)],
            idempotency_key=f"tracker-sync:{connection}:{mode}:{new_id()}",
            created_by=ctx.actor,
        )
        return job.id
