"""stack landing request

Revision ID: 0004
Revises: 0003
Create Date: 2026-10-17 18:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0004"
down_revision: str | Sequence[str] | None = "0003"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    # Existing stacks have no pending landing request, so the column is nullable.
    with op.batch_alter_table("stacks", schema=None) as batch_op:
        batch_op.add_column(sa.Column("land_request", sa.Text(), nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("stacks", schema=None) as batch_op:
        batch_op.drop_column("land_request")
