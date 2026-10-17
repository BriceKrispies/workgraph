"""UTC time and identifier helpers.

All timestamps in Workgraph are timezone-aware UTC. SQLite stores them as ISO-8601
strings via :class:`workgraph.persistence.types.UTCDateTime`.
"""

from __future__ import annotations

import uuid
from datetime import UTC, datetime


def utcnow() -> datetime:
    return datetime.now(UTC)


def ensure_utc(value: datetime) -> datetime:
    if value.tzinfo is None:
        return value.replace(tzinfo=UTC)
    return value.astimezone(UTC)


def new_id() -> str:
    """Canonical identifier: a random UUID4 rendered as a 36 character string."""
    return str(uuid.uuid4())


def is_uuid(value: str) -> bool:
    try:
        uuid.UUID(value)
    except (ValueError, AttributeError, TypeError):
        return False
    return True
