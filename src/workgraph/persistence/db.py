"""SQLite engine and transaction management.

* WAL journal mode so readers never block the writer.
* ``busy_timeout`` so concurrent writers wait instead of failing immediately.
* Foreign keys enforced on every connection.
* Write transactions start with ``BEGIN IMMEDIATE`` so the write lock is taken up
  front, which avoids SQLite's deferred lock-upgrade deadlocks between workers.

Transactions are deliberately short. Callers must never hold a write session while
running external programs or model calls.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from sqlalchemy import Engine, create_engine, event
from sqlalchemy.orm import Session, sessionmaker
from sqlalchemy.pool import NullPool

BUSY_TIMEOUT_MS = 15_000


def _configure_connection(dbapi_conn: Any, _record: Any) -> None:
    # Hand transaction control to SQLAlchemy's "begin" hook below.
    dbapi_conn.isolation_level = None
    cursor = dbapi_conn.cursor()
    cursor.execute("PRAGMA journal_mode=WAL")
    cursor.execute(f"PRAGMA busy_timeout={BUSY_TIMEOUT_MS}")
    cursor.execute("PRAGMA foreign_keys=ON")
    cursor.execute("PRAGMA synchronous=NORMAL")
    cursor.close()


def _begin(conn: Any) -> None:
    mode = "IMMEDIATE" if conn.get_execution_options().get("workgraph_write") else "DEFERRED"
    conn.exec_driver_sql(f"BEGIN {mode}")


def create_sqlite_engine(path: Path) -> Engine:
    path.parent.mkdir(parents=True, exist_ok=True)
    engine = create_engine(
        f"sqlite:///{path.as_posix()}",
        poolclass=NullPool,
        connect_args={"timeout": BUSY_TIMEOUT_MS / 1000, "check_same_thread": False},
    )
    event.listen(engine, "connect", _configure_connection)
    event.listen(engine, "begin", _begin)
    return engine


class Database:
    """Owns the engine and hands out sessions with explicit read/write intent."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.engine = create_sqlite_engine(path)
        self._factory = sessionmaker(self.engine, expire_on_commit=False, autoflush=True)

    @contextmanager
    def session(self, *, write: bool = False) -> Generator[Session]:
        """Yield a session in its own transaction; commit on success, roll back on error."""
        session = self._factory()
        try:
            session.connection(execution_options={"workgraph_write": write})
            yield session
            if write:
                session.commit()
            # Read sessions are simply closed: close() detaches loaded objects without
            # expiring them (rollback() would), so callers may keep using their data.
        except BaseException:
            session.rollback()
            raise
        finally:
            session.close()

    def raw_connection(self) -> sqlite3.Connection:
        """A plain sqlite3 connection (used for backups and analytical extraction)."""
        conn = sqlite3.connect(self.path, timeout=BUSY_TIMEOUT_MS / 1000)
        conn.execute(f"PRAGMA busy_timeout={BUSY_TIMEOUT_MS}")
        conn.execute("PRAGMA foreign_keys=ON")
        return conn

    def dispose(self) -> None:
        self.engine.dispose()
