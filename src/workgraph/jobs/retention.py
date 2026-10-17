"""Retention and compaction for verbose telemetry.

Only *telemetry payloads* are compacted: raw hook payloads and run/verification log
files older than the configured retention window. The records themselves (tasks,
attempts, events, tool-event summaries, artifact metadata) are never deleted;
purged items keep their metadata and a ``purged_at`` timestamp.
"""

from __future__ import annotations

from datetime import timedelta
from typing import TYPE_CHECKING, Any

from sqlalchemy import select, update

from workgraph.core.clock import utcnow
from workgraph.core.fsutil import is_within
from workgraph.persistence.orm import Artifact, HookEvent

if TYPE_CHECKING:
    from workgraph.app import Workgraph


def compact(app: Workgraph, *, dry_run: bool = False) -> dict[str, Any]:
    config = app.config.retention
    now = utcnow()
    payload_cutoff = now - timedelta(days=config.telemetry_payload_days)
    log_cutoff = now - timedelta(days=config.run_log_days)
    report: dict[str, Any] = {"hook_payloads": 0, "artifacts": 0, "bytes_freed": 0, "dry_run": dry_run}
    with app.db.session(write=not dry_run) as session:
        stale = session.scalars(
            select(HookEvent.id).where(
                HookEvent.received_at < payload_cutoff,
                HookEvent.payload.is_not(None),
                HookEvent.payload_purged_at.is_(None),
            )
        ).all()
        report["hook_payloads"] = len(stale)
        if stale and not dry_run:
            session.execute(
                update(HookEvent)
                .where(HookEvent.id.in_(list(stale)))
                .values(payload=None, payload_purged_at=now)
            )
        artifacts = list(
            session.scalars(
                select(Artifact).where(
                    Artifact.retention == "telemetry",
                    Artifact.created_at < log_cutoff,
                    Artifact.purged_at.is_(None),
                )
            )
        )
        root = app.paths.artifacts_dir
        for artifact in artifacts:
            path = root / artifact.path
            if not is_within(path, root):
                continue
            report["artifacts"] += 1
            report["bytes_freed"] += artifact.size_bytes
            if not dry_run:
                if path.exists():
                    path.unlink()
                artifact.purged_at = now
    return report
