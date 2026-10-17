"""Programmatic Alembic migrations (``workgraph init`` / ``workgraph db upgrade``)."""

from __future__ import annotations

from pathlib import Path

from alembic import command
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory

from workgraph.persistence.db import Database

MIGRATIONS_DIR = Path(__file__).parent / "migrations"


def _config(db: Database) -> Config:
    cfg = Config()
    cfg.set_main_option("script_location", str(MIGRATIONS_DIR))
    cfg.set_main_option("workgraph.db_path", str(db.path))
    return cfg


def head_revision() -> str | None:
    script = ScriptDirectory(str(MIGRATIONS_DIR))
    return script.get_current_head()


def current_revision(db: Database) -> str | None:
    with db.engine.connect() as conn:
        return MigrationContext.configure(conn).get_current_revision()


def upgrade(db: Database, revision: str = "head") -> None:
    cfg = _config(db)
    with db.engine.begin() as conn:
        cfg.attributes["connection"] = conn
        command.upgrade(cfg, revision)


def downgrade(db: Database, revision: str) -> None:
    cfg = _config(db)
    with db.engine.begin() as conn:
        cfg.attributes["connection"] = conn
        command.downgrade(cfg, revision)


def is_up_to_date(db: Database) -> bool:
    return current_revision(db) == head_revision()
