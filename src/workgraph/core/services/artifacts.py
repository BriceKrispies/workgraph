"""Artifact storage: files on disk under the data directory, metadata in SQLite."""

from __future__ import annotations

import hashlib
import mimetypes
import re
from pathlib import Path
from typing import Any

from sqlalchemy.orm import Session

from workgraph.core.clock import new_id
from workgraph.core.errors import NotFoundError
from workgraph.core.fsutil import atomic_write_bytes, require_within
from workgraph.persistence.orm import Artifact

_NAME_RE = re.compile(r"[^A-Za-z0-9._-]+")


def safe_name(name: str) -> str:
    cleaned = _NAME_RE.sub("_", name).strip("._") or "artifact"
    return cleaned[:150]


def artifact_dir(root: Path, workspace_id: str, artifact_id: str) -> Path:
    return root / workspace_id / artifact_id


def store_bytes(
    session: Session,
    root: Path,
    *,
    workspace_id: str,
    kind: str,
    name: str,
    content: bytes,
    task_id: str | None = None,
    attempt_id: str | None = None,
    media_type: str | None = None,
    retention: str = "keep",
    metadata: dict[str, Any] | None = None,
) -> Artifact:
    artifact_id = new_id()
    filename = safe_name(name)
    target = artifact_dir(root, workspace_id, artifact_id) / filename
    atomic_write_bytes(target, content)
    artifact = Artifact(
        id=artifact_id,
        workspace_id=workspace_id,
        task_id=task_id,
        attempt_id=attempt_id,
        kind=kind,
        name=filename,
        media_type=media_type or mimetypes.guess_type(filename)[0] or "application/octet-stream",
        path=target.relative_to(root).as_posix(),
        size_bytes=len(content),
        sha256=hashlib.sha256(content).hexdigest(),
        retention=retention,
        metadata_=metadata or {},
    )
    session.add(artifact)
    session.flush()
    return artifact


def register_file(
    session: Session,
    root: Path,
    file: Path,
    *,
    workspace_id: str,
    kind: str,
    task_id: str | None = None,
    attempt_id: str | None = None,
    retention: str = "telemetry",
    metadata: dict[str, Any] | None = None,
) -> Artifact:
    """Record metadata for a file already written inside the artifacts directory."""
    resolved = require_within(file, root, what="artifact")
    data = resolved.read_bytes() if resolved.exists() else b""
    artifact = Artifact(
        workspace_id=workspace_id,
        task_id=task_id,
        attempt_id=attempt_id,
        kind=kind,
        name=resolved.name,
        media_type=mimetypes.guess_type(resolved.name)[0] or "text/plain",
        path=resolved.relative_to(root.resolve()).as_posix(),
        size_bytes=len(data),
        sha256=hashlib.sha256(data).hexdigest(),
        retention=retention,
        metadata_=metadata or {},
    )
    session.add(artifact)
    session.flush()
    return artifact


def read_artifact(session: Session, root: Path, artifact_id: str) -> tuple[Artifact, bytes | None]:
    artifact = session.get(Artifact, artifact_id)
    if artifact is None:
        raise NotFoundError(f"artifact {artifact_id} not found")
    if artifact.purged_at is not None:
        return artifact, None
    path = require_within(root / artifact.path, root, what="artifact")
    return artifact, path.read_bytes() if path.exists() else None
