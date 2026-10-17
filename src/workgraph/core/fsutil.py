"""Filesystem helpers: atomic writes and path boundary checks."""

from __future__ import annotations

import contextlib
import os
import tempfile
from pathlib import Path

from workgraph.core.errors import ValidationFailed


def atomic_write_bytes(path: Path, data: bytes) -> None:
    """Write ``data`` to ``path`` so readers never observe a partial file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        tmp.replace(path)
    except BaseException:
        with contextlib.suppress(OSError):
            tmp.unlink()
        raise


def atomic_write_text(path: Path, text: str) -> None:
    atomic_write_bytes(path, text.encode("utf-8"))


def is_within(child: Path, parent: Path) -> bool:
    try:
        child.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def require_within(child: Path, parent: Path, *, what: str = "path") -> Path:
    resolved = child.resolve()
    if not is_within(resolved, parent):
        raise ValidationFailed(
            f"{what} {resolved} escapes the permitted boundary {parent.resolve()}",
            details={"path": str(resolved), "boundary": str(parent.resolve())},
        )
    return resolved


def safe_relative_path(value: str) -> Path:
    """Validate a repository-relative path received from untrusted input."""
    if not value or "\x00" in value:
        raise ValidationFailed("empty or NUL-containing path")
    candidate = Path(value)
    if (
        candidate.is_absolute()
        or candidate.drive
        or candidate.root
        or any(p == ".." for p in candidate.parts)
    ):
        raise ValidationFailed(f"path {value!r} must be relative and must not traverse upwards")
    return candidate
