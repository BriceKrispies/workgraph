"""Column types shared by the ORM schema."""

from __future__ import annotations

import json
from datetime import UTC, datetime
from typing import Any

from sqlalchemy import Dialect, String, Text
from sqlalchemy.types import TypeDecorator


class UTCDateTime(TypeDecorator[datetime]):
    """Timezone-aware UTC datetimes persisted as sortable ISO-8601 strings.

    Values are normalised to microsecond precision with a fixed ``+00:00`` suffix so
    that lexical ordering in SQLite equals chronological ordering.
    """

    impl = String(32)
    cache_ok = True

    def process_bind_param(self, value: datetime | None, dialect: Dialect) -> str | None:
        if value is None:
            return None
        if value.tzinfo is None:
            value = value.replace(tzinfo=UTC)
        return value.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%S.%f+00:00")

    def process_result_value(self, value: str | None, dialect: Dialect) -> datetime | None:
        if value is None:
            return None
        parsed = datetime.fromisoformat(value)
        return parsed if parsed.tzinfo else parsed.replace(tzinfo=UTC)


class JSONText(TypeDecorator[Any]):
    """JSON stored as TEXT with deterministic key ordering."""

    impl = Text
    cache_ok = True

    def process_bind_param(self, value: Any, dialect: Dialect) -> str | None:
        if value is None:
            return None
        return json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)

    def process_result_value(self, value: str | None, dialect: Dialect) -> Any:
        if value is None:
            return None
        return json.loads(value)
