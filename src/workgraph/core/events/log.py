"""Append-only domain event log.

Events are written inside the same SQLAlchemy transaction as the current-state
change they describe, so either both persist or neither does. The ``events``
table rejects UPDATE/DELETE at the database level (see migration ``0001``).
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.clock import new_id, utcnow
from workgraph.core.context import RequestContext
from workgraph.persistence.orm import Event

# Current schema version per event type. Bump when the ``data`` shape changes and
# register an upcaster that converts the previous version.
EVENT_SCHEMA_VERSIONS: dict[str, int] = {
    "task.created": 2,
}
DEFAULT_SCHEMA_VERSION = 1

Upcaster = Callable[[dict[str, Any]], dict[str, Any]]


def _task_created_v1_to_v2(data: dict[str, Any]) -> dict[str, Any]:
    # v1 stored tags as a comma-separated string and had no acceptance criteria.
    upgraded = dict(data)
    tags = upgraded.get("tags", "")
    if isinstance(tags, str):
        upgraded["tags"] = [t.strip() for t in tags.split(",") if t.strip()]
    upgraded.setdefault("acceptance_criteria", [])
    return upgraded


UPCASTERS: dict[tuple[str, int], Upcaster] = {
    ("task.created", 1): _task_created_v1_to_v2,
}


@dataclass(frozen=True)
class EventRecord:
    seq: int
    id: str
    workspace_id: str | None
    event_type: str
    entity_type: str
    entity_id: str
    actor: str
    actor_kind: str
    occurred_at: datetime
    correlation_id: str
    causation_id: str | None
    attempt_id: str | None
    schema_version: int
    data: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return {
            "seq": self.seq,
            "id": self.id,
            "workspace_id": self.workspace_id,
            "event_type": self.event_type,
            "entity_type": self.entity_type,
            "entity_id": self.entity_id,
            "actor": self.actor,
            "actor_kind": self.actor_kind,
            "occurred_at": self.occurred_at.isoformat(),
            "correlation_id": self.correlation_id,
            "causation_id": self.causation_id,
            "attempt_id": self.attempt_id,
            "schema_version": self.schema_version,
            "data": self.data,
        }


def upcast(event_type: str, version: int, data: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    """Convert an event payload to the current schema version for its type."""
    target = EVENT_SCHEMA_VERSIONS.get(event_type, DEFAULT_SCHEMA_VERSION)
    while version < target:
        step = UPCASTERS.get((event_type, version))
        if step is None:
            break
        data = step(data)
        version += 1
    return version, data


def to_record(event: Event) -> EventRecord:
    version, data = upcast(event.event_type, event.schema_version, event.data or {})
    return EventRecord(
        seq=event.seq,
        id=event.id,
        workspace_id=event.workspace_id,
        event_type=event.event_type,
        entity_type=event.entity_type,
        entity_id=event.entity_id,
        actor=event.actor,
        actor_kind=event.actor_kind,
        occurred_at=event.occurred_at,
        correlation_id=event.correlation_id,
        causation_id=event.causation_id,
        attempt_id=event.attempt_id,
        schema_version=version,
        data=data,
    )


def record_event(
    session: Session,
    ctx: RequestContext,
    *,
    event_type: str,
    entity_type: str,
    entity_id: str,
    workspace_id: str | None,
    data: dict[str, Any] | None = None,
    attempt_id: str | None = None,
    occurred_at: datetime | None = None,
) -> Event:
    payload = dict(data or {})
    if ctx.automation_depth > 0:
        # Recorded so the automation engine can bound recursive rule chains.
        payload["_automation"] = {"depth": ctx.automation_depth, "actor": ctx.actor}
    event = Event(
        id=new_id(),
        workspace_id=workspace_id,
        event_type=event_type,
        entity_type=entity_type,
        entity_id=entity_id,
        actor=ctx.actor,
        actor_kind=str(ctx.actor_kind),
        occurred_at=occurred_at or utcnow(),
        correlation_id=ctx.correlation_id,
        causation_id=ctx.causation_id,
        attempt_id=attempt_id or ctx.attempt_id,
        schema_version=EVENT_SCHEMA_VERSIONS.get(event_type, DEFAULT_SCHEMA_VERSION),
        data=payload,
    )
    session.add(event)
    session.flush()
    return event


def query_events(
    session: Session,
    *,
    workspace_id: str | None = None,
    entity_type: str | None = None,
    entity_ids: Sequence[str] | None = None,
    event_types: Sequence[str] | None = None,
    event_type_prefix: str | None = None,
    actor: str | None = None,
    attempt_id: str | None = None,
    correlation_id: str | None = None,
    after_seq: int | None = None,
    since: datetime | None = None,
    until: datetime | None = None,
    limit: int = 200,
    descending: bool = False,
) -> list[EventRecord]:
    stmt = select(Event)
    if workspace_id is not None:
        stmt = stmt.where(Event.workspace_id == workspace_id)
    if entity_type is not None:
        stmt = stmt.where(Event.entity_type == entity_type)
    if entity_ids:
        stmt = stmt.where(Event.entity_id.in_(list(entity_ids)))
    if event_types:
        stmt = stmt.where(Event.event_type.in_(list(event_types)))
    if event_type_prefix:
        stmt = stmt.where(Event.event_type.startswith(event_type_prefix))
    if actor is not None:
        stmt = stmt.where(Event.actor == actor)
    if attempt_id is not None:
        stmt = stmt.where(Event.attempt_id == attempt_id)
    if correlation_id is not None:
        stmt = stmt.where(Event.correlation_id == correlation_id)
    if after_seq is not None:
        stmt = stmt.where(Event.seq > after_seq)
    if since is not None:
        stmt = stmt.where(Event.occurred_at >= since)
    if until is not None:
        stmt = stmt.where(Event.occurred_at < until)
    stmt = stmt.order_by(Event.seq.desc() if descending else Event.seq.asc()).limit(limit)
    return [to_record(e) for e in session.scalars(stmt)]


def latest_seq(session: Session) -> int:
    from sqlalchemy import func

    return int(session.scalar(select(func.coalesce(func.max(Event.seq), 0))) or 0)
