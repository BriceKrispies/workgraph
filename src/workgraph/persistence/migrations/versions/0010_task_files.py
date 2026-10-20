"""task files: structured paths (and anchors) a task expects to change

Revision ID: 0010
Revises: 0009
Create Date: 2026-10-20 12:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0010"
down_revision: str | Sequence[str] | None = "0009"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "task_files",
        sa.Column("task_id", sa.String(), nullable=False),
        sa.Column("position", sa.Integer(), nullable=False),
        sa.Column("path", sa.String(length=1000), nullable=False),
        sa.Column("anchor", sa.String(length=300), nullable=True),
        sa.Column("note", sa.Text(), nullable=False),
        sa.ForeignKeyConstraint(
            ["task_id"],
            ["tasks.id"],
            name=op.f("fk_task_files_task_id_tasks"),
            ondelete="CASCADE",
        ),
        sa.PrimaryKeyConstraint("task_id", "position", name=op.f("pk_task_files")),
    )
    with op.batch_alter_table("task_files", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_task_files_path"), ["path"], unique=False)


def downgrade() -> None:
    with op.batch_alter_table("task_files", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_task_files_path"))
    op.drop_table("task_files")
