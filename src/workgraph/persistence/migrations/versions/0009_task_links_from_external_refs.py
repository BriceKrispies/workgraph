"""move tasks.external_refs into task_links

Revision ID: 0009
Revises: 0008
Create Date: 2026-10-18 12:00:00.000000
"""

import json
import uuid
from collections.abc import Sequence
from datetime import UTC, datetime

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0009"
down_revision: str | Sequence[str] | None = "0008"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

UTC_DT = workgraph.persistence.types.UTCDateTime
JSON = workgraph.persistence.types.JSONText


def upgrade() -> None:
    bind = op.get_bind()
    now = datetime.now(UTC)
    rows: list[dict[str, object]] = []
    for task_id, raw in bind.execute(sa.text("SELECT id, external_refs FROM tasks")).all():
        seen: set[str] = set()
        for ref in json.loads(raw) if raw else []:
            if not isinstance(ref, dict):
                continue
            system = str(ref.get("system") or "")
            ref_id = str(ref.get("id") or "")
            normalized = f"{system}:{ref_id}"
            if not system or not ref_id or normalized in seen:
                continue
            seen.add(normalized)
            rows.append(
                {
                    "id": str(uuid.uuid4()),
                    "task_id": task_id,
                    "kind": "external_ref",
                    "provider": system,
                    "url": str(ref.get("url") or ""),
                    "normalized_url": normalized,
                    "repo_full_name": "",
                    "number": None,
                    "repository_id": None,
                    "sources": ["migrated"],
                    "title": ref_id,
                    "created_by": "system:migration",
                    "created_at": now,
                    "updated_at": now,
                }
            )
    # Dropping the column rebuilds `tasks` on SQLite, which would cascade-delete any task_links rows,
    # so the links are written only after the rebuild.
    with op.batch_alter_table("tasks", schema=None) as batch_op:
        batch_op.drop_column("external_refs")
    if rows:
        task_links = sa.table(
            "task_links",
            sa.column("id", sa.String()),
            sa.column("task_id", sa.String()),
            sa.column("kind", sa.String()),
            sa.column("provider", sa.String()),
            sa.column("url", sa.String()),
            sa.column("normalized_url", sa.String()),
            sa.column("repo_full_name", sa.String()),
            sa.column("number", sa.Integer()),
            sa.column("repository_id", sa.String()),
            sa.column("sources", JSON()),
            sa.column("title", sa.Text()),
            sa.column("created_by", sa.String()),
            sa.column("created_at", UTC_DT()),
            sa.column("updated_at", UTC_DT()),
        )
        op.bulk_insert(task_links, rows)


def downgrade() -> None:
    # Remove the migrated links first; a plain ADD COLUMN avoids rebuilding `tasks` and cascading other links.
    op.execute(sa.text("DELETE FROM task_links WHERE kind = 'external_ref'"))
    op.add_column("tasks", sa.Column("external_refs", JSON(), nullable=False, server_default=sa.text("'[]'")))
