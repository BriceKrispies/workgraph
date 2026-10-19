"""tracker items and task links

Revision ID: 0008
Revises: 0007
Create Date: 2026-10-19 12:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0008"
down_revision: str | Sequence[str] | None = "0007"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

UTC = workgraph.persistence.types.UTCDateTime
JSON = workgraph.persistence.types.JSONText


def upgrade() -> None:
    op.create_table(
        "external_items",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("workspace_id", sa.String(), nullable=False),
        sa.Column("task_id", sa.String(), nullable=False),
        sa.Column("connection", sa.String(length=100), nullable=False),
        sa.Column("external_id", sa.String(length=200), nullable=False),
        sa.Column("external_key", sa.String(length=64), nullable=False),
        sa.Column("url", sa.String(length=1000), nullable=False),
        sa.Column("tracker_status", sa.String(length=200), nullable=False),
        sa.Column("tracker_status_category", sa.String(length=32), nullable=False),
        sa.Column("tracker_resolution", sa.String(length=200), nullable=True),
        sa.Column("remote_updated_at", UTC(length=32), nullable=True),
        sa.Column("snapshot", JSON(), nullable=False),
        sa.Column("snapshot_hash", sa.String(length=64), nullable=False),
        sa.Column("synced_at", UTC(length=32), nullable=True),
        sa.Column("missing_since", UTC(length=32), nullable=True),
        sa.Column("created_at", UTC(length=32), nullable=False),
        sa.Column("updated_at", UTC(length=32), nullable=False),
        sa.ForeignKeyConstraint(
            ["workspace_id"],
            ["workspaces.id"],
            name=op.f("fk_external_items_workspace_id_workspaces"),
            ondelete="RESTRICT",
        ),
        sa.ForeignKeyConstraint(
            ["task_id"], ["tasks.id"], name=op.f("fk_external_items_task_id_tasks"), ondelete="CASCADE"
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_external_items")),
        sa.UniqueConstraint("connection", "external_id", name=op.f("uq_external_items_connection")),
        sa.UniqueConstraint("task_id", name=op.f("uq_external_items_task_id")),
    )
    with op.batch_alter_table("external_items", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_external_items_workspace_id"), ["workspace_id"], unique=False)
        batch_op.create_index(batch_op.f("ix_external_items_external_key"), ["external_key"], unique=False)

    op.create_table(
        "task_links",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("task_id", sa.String(), nullable=False),
        sa.Column("kind", sa.String(length=32), nullable=False),
        sa.Column("provider", sa.String(length=100), nullable=False),
        sa.Column("url", sa.String(length=1000), nullable=False),
        sa.Column("normalized_url", sa.String(length=1000), nullable=False),
        sa.Column("repo_full_name", sa.String(length=300), nullable=False),
        sa.Column("number", sa.Integer(), nullable=True),
        sa.Column("repository_id", sa.String(), nullable=True),
        sa.Column("sources", JSON(), nullable=False),
        sa.Column("title", sa.Text(), nullable=False),
        sa.Column("created_by", sa.String(length=200), nullable=False),
        sa.Column("created_at", UTC(length=32), nullable=False),
        sa.Column("updated_at", UTC(length=32), nullable=False),
        sa.ForeignKeyConstraint(
            ["task_id"], ["tasks.id"], name=op.f("fk_task_links_task_id_tasks"), ondelete="CASCADE"
        ),
        sa.ForeignKeyConstraint(
            ["repository_id"],
            ["repositories.id"],
            name=op.f("fk_task_links_repository_id_repositories"),
            ondelete="SET NULL",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_task_links")),
        sa.UniqueConstraint("task_id", "kind", "normalized_url", name=op.f("uq_task_links_task_id")),
    )
    with op.batch_alter_table("task_links", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_task_links_task_id"), ["task_id"], unique=False)
        batch_op.create_index(batch_op.f("ix_task_links_normalized_url"), ["normalized_url"], unique=False)
        batch_op.create_index(batch_op.f("ix_task_links_repository_id"), ["repository_id"], unique=False)


def downgrade() -> None:
    with op.batch_alter_table("task_links", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_task_links_repository_id"))
        batch_op.drop_index(batch_op.f("ix_task_links_normalized_url"))
        batch_op.drop_index(batch_op.f("ix_task_links_task_id"))
    op.drop_table("task_links")
    with op.batch_alter_table("external_items", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_external_items_external_key"))
        batch_op.drop_index(batch_op.f("ix_external_items_workspace_id"))
    op.drop_table("external_items")
