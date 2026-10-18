"""subagent telemetry

Revision ID: 0005
Revises: 0004
Create Date: 2026-10-18 09:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0005"
down_revision: str | Sequence[str] | None = "0004"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    # Existing subagent runs have no transcript telemetry yet, so every column is nullable.
    with op.batch_alter_table("subagent_runs", schema=None) as batch_op:
        batch_op.add_column(sa.Column("requested_model", sa.String(length=200), nullable=True))
        batch_op.add_column(sa.Column("model", sa.String(length=200), nullable=True))
        batch_op.add_column(sa.Column("usage", workgraph.persistence.types.JSONText(), nullable=True))
        batch_op.add_column(sa.Column("telemetry", workgraph.persistence.types.JSONText(), nullable=True))
        batch_op.add_column(sa.Column("task_id", sa.String(length=36), nullable=True))
        batch_op.create_index(batch_op.f("ix_subagent_runs_task_id"), ["task_id"], unique=False)
        batch_op.create_foreign_key(
            batch_op.f("fk_subagent_runs_task_id_tasks"), "tasks", ["task_id"], ["id"], ondelete="SET NULL"
        )


def downgrade() -> None:
    with op.batch_alter_table("subagent_runs", schema=None) as batch_op:
        batch_op.drop_constraint(batch_op.f("fk_subagent_runs_task_id_tasks"), type_="foreignkey")
        batch_op.drop_index(batch_op.f("ix_subagent_runs_task_id"))
        batch_op.drop_column("task_id")
        batch_op.drop_column("telemetry")
        batch_op.drop_column("usage")
        batch_op.drop_column("model")
        batch_op.drop_column("requested_model")
