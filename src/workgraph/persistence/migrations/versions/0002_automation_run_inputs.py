"""automation run inputs

Revision ID: 0002
Revises: 0001
Create Date: 2026-10-17 10:18:40.957301
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0002"
down_revision: str | Sequence[str] | None = "0001"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    # Existing rows receive an empty object, so the column can be NOT NULL immediately.
    with op.batch_alter_table("automation_runs", schema=None) as batch_op:
        batch_op.add_column(
            sa.Column("inputs", workgraph.persistence.types.JSONText(), nullable=False, server_default="{}")
        )


def downgrade() -> None:
    with op.batch_alter_table("automation_runs", schema=None) as batch_op:
        batch_op.drop_column("inputs")
