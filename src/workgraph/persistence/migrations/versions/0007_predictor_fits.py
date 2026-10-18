"""predictor fits

Revision ID: 0007
Revises: 0006
Create Date: 2026-10-18 12:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0007"
down_revision: str | Sequence[str] | None = "0006"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "predictor_fits",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("workspace_id", sa.String(), nullable=False),
        sa.Column("model", sa.String(length=200), nullable=False),
        sa.Column("created_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.Column("data_cutoff", workgraph.persistence.types.UTCDateTime(length=32), nullable=True),
        sa.Column("trial_ids", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("feature_names", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("repositories", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("coef", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("penalty", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("n", sa.Integer(), nullable=False),
        sa.Column("converged", sa.Boolean(), nullable=False),
        sa.Column("metrics", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("samples", workgraph.persistence.types.JSONText(), nullable=False),
        sa.ForeignKeyConstraint(
            ["workspace_id"],
            ["workspaces.id"],
            name=op.f("fk_predictor_fits_workspace_id_workspaces"),
            ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_predictor_fits")),
    )
    with op.batch_alter_table("predictor_fits", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_predictor_fits_workspace_id"), ["workspace_id"], unique=False)


def downgrade() -> None:
    with op.batch_alter_table("predictor_fits", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_predictor_fits_workspace_id"))
    op.drop_table("predictor_fits")
