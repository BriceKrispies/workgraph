"""Alembic environment. Migrations are executed programmatically by
:mod:`workgraph.persistence.migrate`; the connection is passed via ``config.attributes``."""

from alembic import context
from sqlalchemy import Connection

from workgraph.persistence.orm import Base

config = context.config
target_metadata = Base.metadata


def _run(connection: Connection) -> None:
    context.configure(
        connection=connection,
        target_metadata=target_metadata,
        render_as_batch=True,
        compare_type=True,
    )
    with context.begin_transaction():
        context.run_migrations()


connection = config.attributes.get("connection")
if connection is None:
    from pathlib import Path

    from workgraph.persistence.db import create_sqlite_engine

    engine = create_sqlite_engine(Path(config.get_main_option("workgraph.db_path") or "workgraph.db"))
    with engine.connect() as conn:
        _run(conn)
        conn.commit()
else:
    _run(connection)
