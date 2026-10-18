"""Replay pool: turn a repository's commits into replay tasks, one task per commit.

The service wraps :mod:`workgraph.experiments.replay` with workspace resolution, the repository's
primary checkout, de-duplication against existing replay tasks, and task creation.
"""

from __future__ import annotations

import shlex
from collections.abc import Mapping, Sequence
from dataclasses import replace
from datetime import datetime
from pathlib import Path
from typing import Any

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ValidationFailed
from workgraph.core.models.schemas import TaskCreate, TaskFilter
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.experiments import replay
from workgraph.experiments.replay import ReplayCandidate, ReplayConfig
from workgraph.security.permissions import Permission, require

REPLAY_TAG = "replay"
POOL_TAG = "experiment-pool"
TITLE_MAX = 500


class ReplayPool(Service):
    def _workspace_id(self, workspace: str | None) -> str:
        if workspace is None:
            return self.app.default_workspace_id()
        with self.db.session() as session:
            return resolve.workspace(session, workspace).id

    def _checkout(self, workspace: str | None, repository: str) -> Path:
        ws_id = self._workspace_id(workspace)
        with self.db.session() as session:
            repo = resolve.repository(session, ws_id, repository)
            checkout = primary_checkout(repo)
            if checkout is None:
                raise ValidationFailed(f"repository {repo.name!r} has no available checkout")
            return Path(checkout.path)

    def scan(
        self,
        ctx: RequestContext,
        workspace: str | None,
        repository: str,
        *,
        rev: str = "HEAD",
        limit: int | None = None,
        min_lines: int = 1,
        max_lines: int | None = None,
        include_excluded: bool = False,
    ) -> list[ReplayCandidate]:
        require(ctx, Permission.TASK_READ)
        path = self._checkout(workspace, repository)
        return replay.scan(
            path,
            rev=rev,
            limit=limit,
            min_lines=min_lines,
            max_lines=max_lines,
            include_excluded=include_excluded,
        )

    def _replay_tasks_by_sha(self, ctx: RequestContext, workspace: str | None) -> dict[str, str]:
        """Map full commit sha to task key for every workspace task tagged ``replay``."""
        found: dict[str, str] = {}
        offset = 0
        while True:
            page = self.app.tasks.list(
                ctx, workspace, TaskFilter(tags=[REPLAY_TAG], limit=500, offset=offset, include_archived=True)
            )
            for task in page.items:
                meta = task.metadata.get("replay", {})
                sha = meta.get("sha")
                # A task built under another classifier is not a duplicate: its tests and size differ.
                if (
                    isinstance(sha, str)
                    and meta.get("classifier", "replay-classifier-1") == replay.CLASSIFIER
                ):
                    found.setdefault(sha, task.key)
            if page.next_offset is None:
                return found
            offset = page.next_offset

    def create(
        self,
        ctx: RequestContext,
        workspace: str | None,
        repository: str,
        shas: Sequence[str],
        *,
        validate: bool = False,
        oracle_template: Sequence[str] | None = None,
        env: Mapping[str, str] | None = None,
        timeout: float = 1800.0,
    ) -> list[dict[str, Any]]:
        require(ctx, Permission.EXPERIMENT_WRITE)
        path = self._checkout(workspace, repository)
        config = ReplayConfig.for_repository(path)
        if oracle_template is not None:
            config = replace(config, oracle_template=tuple(oracle_template))
        existing = self._replay_tasks_by_sha(ctx, workspace)
        results: list[dict[str, Any]] = []
        for sha in shas:
            candidate = replay.analyze_commit(path, sha, config)
            if candidate.excluded:
                results.append(_result(candidate.sha, "skipped", None, ",".join(candidate.excluded)))
                continue
            if candidate.sha in existing:
                results.append(_result(candidate.sha, "exists", existing[candidate.sha], None))
                continue
            base_ref = replay.create_base(path, candidate)
            validation: replay.Validation | None = None
            if validate:
                validation = replay.validate(path, candidate, base_ref, timeout=timeout, env=env)
                if not validation.ok:
                    results.append(_result(candidate.sha, "rejected", None, validation.reason))
                    continue
            task = self.app.tasks.create(
                ctx, workspace, _task_create(candidate, repository, base_ref, validation)
            )
            existing[candidate.sha] = task.key
            results.append(_result(candidate.sha, "created", task.key, None))
        return results


def _result(sha: str, status: str, task_key: str | None, reason: str | None) -> dict[str, Any]:
    return {"sha": sha, "status": status, "task_key": task_key, "reason": reason}


def _task_create(
    candidate: ReplayCandidate,
    repository: str,
    base_ref: str,
    validation: replay.Validation | None,
) -> TaskCreate:
    subject = candidate.subject
    oracle = [list(command) for command in candidate.oracle]
    description = "\n".join(
        [
            f"Replay of commit `{candidate.sha}`.",
            "",
            subject,
            "",
            candidate.body,
            "",
            "## Tests",
            *[f"- `{path}`" for path in candidate.test_paths],
            "",
            "## Oracle",
            *[f"- `{shlex.join(command)}`" for command in oracle],
            "",
            f"Base branch: `{base_ref}`",
        ]
    )
    validated_at: datetime | None = utcnow() if validation is not None else None
    metadata = {
        "replay": {
            "sha": candidate.sha,
            "classifier": replay.CLASSIFIER,
            "parent": candidate.parent,
            "base_ref": base_ref,
            "test_paths": list(candidate.test_paths),
            "source_paths": list(candidate.source_paths),
            "oracle": oracle,
            "features": candidate.features.as_dict(),
            "validated": validation is not None,
            "validation": None
            if validation is None
            else {"base": list(validation.base), "solution": list(validation.solution)},
            "validated_at": validated_at.isoformat() if validated_at is not None else None,
        }
    }
    return TaskCreate(
        title=f"Replay {candidate.sha[:12]}: {subject}"[:TITLE_MAX],
        description=description,
        tags=[REPLAY_TAG, POOL_TAG],
        repositories=[repository],
        metadata=metadata,
    )
