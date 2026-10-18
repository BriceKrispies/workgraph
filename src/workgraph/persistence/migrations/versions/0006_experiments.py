"""experiments and experiment trials

Revision ID: 0006
Revises: 0005
Create Date: 2026-10-18 10:00:00.000000
"""

from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

import workgraph.persistence.types

revision: str = "0006"
down_revision: str | Sequence[str] | None = "0005"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "experiments",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("workspace_id", sa.String(), nullable=False),
        sa.Column("slug", sa.String(length=100), nullable=False),
        sa.Column("title", sa.String(length=500), nullable=False),
        sa.Column("question", sa.Text(), nullable=True),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("protocol", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("frozen_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=True),
        sa.Column("created_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.Column("updated_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.ForeignKeyConstraint(
            ["workspace_id"],
            ["workspaces.id"],
            name=op.f("fk_experiments_workspace_id_workspaces"),
            ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_experiments")),
        sa.UniqueConstraint("workspace_id", "slug", name=op.f("uq_experiments_workspace_id")),
    )
    with op.batch_alter_table("experiments", schema=None) as batch_op:
        batch_op.create_index(batch_op.f("ix_experiments_workspace_id"), ["workspace_id"], unique=False)

    op.create_table(
        "experiment_trials",
        sa.Column("id", sa.String(), nullable=False),
        sa.Column("experiment_id", sa.String(), nullable=False),
        sa.Column("task_id", sa.String(), nullable=False),
        sa.Column("attempt_id", sa.String(length=36), nullable=True),
        sa.Column("subagent_run_id", sa.String(length=36), nullable=True),
        sa.Column("arm", sa.String(length=64), nullable=False),
        sa.Column("requested_model", sa.String(length=200), nullable=False),
        sa.Column("actual_model", sa.String(length=200), nullable=True),
        sa.Column("phase", sa.String(length=16), nullable=False),
        sa.Column("seq", sa.Integer(), nullable=False),
        sa.Column("band", sa.Integer(), nullable=True),
        sa.Column("features", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("base_ref", sa.String(length=300), nullable=True),
        sa.Column("worktree_path", sa.String(length=1000), nullable=True),
        sa.Column("status", sa.String(length=16), nullable=False),
        sa.Column("outcome", sa.String(length=8), nullable=True),
        sa.Column("outcome_detail", workgraph.persistence.types.JSONText(), nullable=False),
        sa.Column("failure_cause", sa.String(length=32), nullable=True),
        sa.Column("failure_cause_source", sa.String(length=16), nullable=True),
        sa.Column("notes", sa.Text(), nullable=True),
        sa.Column("created_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=False),
        sa.Column("started_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=True),
        sa.Column("graded_at", workgraph.persistence.types.UTCDateTime(length=32), nullable=True),
        sa.ForeignKeyConstraint(
            ["experiment_id"],
            ["experiments.id"],
            name=op.f("fk_experiment_trials_experiment_id_experiments"),
            ondelete="CASCADE",
        ),
        sa.ForeignKeyConstraint(
            ["task_id"],
            ["tasks.id"],
            name=op.f("fk_experiment_trials_task_id_tasks"),
            ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("id", name=op.f("pk_experiment_trials")),
        sa.UniqueConstraint("experiment_id", "seq", name=op.f("uq_experiment_trials_experiment_id")),
    )
    with op.batch_alter_table("experiment_trials", schema=None) as batch_op:
        batch_op.create_index(
            batch_op.f("ix_experiment_trials_experiment_id"), ["experiment_id"], unique=False
        )
        batch_op.create_index(batch_op.f("ix_experiment_trials_task_id"), ["task_id"], unique=False)


def downgrade() -> None:
    with op.batch_alter_table("experiment_trials", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_experiment_trials_task_id"))
        batch_op.drop_index(batch_op.f("ix_experiment_trials_experiment_id"))

    op.drop_table("experiment_trials")
    with op.batch_alter_table("experiments", schema=None) as batch_op:
        batch_op.drop_index(batch_op.f("ix_experiments_workspace_id"))

    op.drop_table("experiments")
