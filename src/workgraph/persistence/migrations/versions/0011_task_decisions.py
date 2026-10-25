"""task decisions: structured questions a task is waiting on, with options and the answer

Revision ID: 0011
Revises: 0010
Create Date: 2026-10-25 12:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0011"
down_revision: str | Sequence[str] | None = "0010"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "task_decisions",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("workspace_id", sa.String(), nullable=False),
        sa.Column("task_id", sa.String(), nullable=False),
        sa.Column("question", sa.Text(), nullable=False),
        sa.Column("options", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("chosen_option_id", sa.String(length=64), nullable=True),
        sa.Column("other_text", sa.Text(), nullable=True),
        sa.Column("note", sa.Text(), nullable=True),
        sa.Column("decided_by", sa.String(length=200), nullable=True),
        sa.Column("decided_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=True),
        sa.Column("created_by", sa.String(length=200), nullable=False),
        sa.Column("created_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.Column("updated_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.Column("version", sa.Integer(), nullable=False),
        sa.ForeignKeyConstraint(
            ["task_id"],
            ["tasks.id"],
            name=op.f("fk_task_decisions_task_id_tasks"),
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["workspace_id"],
            ["workspaces.id"],
            name=op.f("fk_task_decisions_workspace_id_workspaces"),
            ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_task_decisions")),
    )
    with op.batch_alter_table("task_decisions", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_task_decisions_status"), ["status"], unique=False)
        batch_op.create_index(batch_op.f("ix_task_decisions_task_id"), ["task_id"], unique=False)
        batch_op.create_index(batch_op.f("ix_task_decisions_workspace_id"), ["workspace_id"], unique=False)


def downgrade() -> None:
    with op.batch_alter_table("task_decisions", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_task_decisions_workspace_id"))
        batch_op.drop_index(batch_op.f("ix_task_decisions_task_id"))
        batch_op.drop_index(batch_op.f("ix_task_decisions_status"))
    op.drop_table("task_decisions")
