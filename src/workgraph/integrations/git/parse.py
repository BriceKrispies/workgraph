"""Parsers for machine-readable Git output (``-z`` formats only)."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import datetime

from workgraph.core.clock import ensure_utc

META_FORMAT = (
    "%x1e%H%x1f%T%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%(trailers:unfold,separator=%x1d)%x1f%B"
)
_HEX = re.compile(r"^[0-9a-f]{40}([0-9a-f]{24})?$")
_NUMSTAT = re.compile(r"^(\d+|-)\t(\d+|-)\t(.*)$", re.DOTALL)
_TRAILER = re.compile(r"^([A-Za-z0-9][A-Za-z0-9-]*):\s*(.*)$")
CHERRY_PICK_RE = re.compile(r"\(cherry picked from commit ([0-9a-f]{7,64})\)")


@dataclass
class CommitMeta:
    sha: str
    tree: str
    parents: list[str]
    author_name: str
    author_email: str
    authored_at: datetime | None
    committer_name: str
    committer_email: str
    committed_at: datetime | None
    trailers: list[dict[str, str]]
    message: str

    @property
    def subject(self) -> str:
        return self.message.split("\n", 1)[0].strip()

    @property
    def cherry_picked_from(self) -> str | None:
        match = CHERRY_PICK_RE.search(self.message)
        return match.group(1) if match else None


@dataclass
class FileChange:
    path: str
    old_path: str | None
    status: str
    similarity: int | None = None
    insertions: int = 0
    deletions: int = 0
    binary: bool = False


@dataclass
class CommitDiff:
    sha: str
    files: list[FileChange] = field(default_factory=list)


def _date(value: str) -> datetime | None:
    value = value.strip()
    if not value:
        return None
    return ensure_utc(datetime.fromisoformat(value))


def parse_trailers(raw: str) -> list[dict[str, str]]:
    trailers: list[dict[str, str]] = []
    for item in raw.split("\x1d"):
        item = item.strip()
        if not item:
            continue
        match = _TRAILER.match(item)
        if match:
            trailers.append({"key": match.group(1), "value": match.group(2).strip()})
    return trailers


def parse_meta(output: str) -> list[CommitMeta]:
    commits: list[CommitMeta] = []
    for record in output.split("\x1e"):
        if not record.strip():
            continue
        fields = record.split("\x1f", 10)
        if len(fields) < 11:
            continue
        sha, tree, parents, an, ae, ad, cn, ce, cd, trailers, body = fields
        commits.append(
            CommitMeta(
                sha=sha.strip(),
                tree=tree.strip(),
                parents=parents.split(),
                author_name=an,
                author_email=ae,
                authored_at=_date(ad),
                committer_name=cn,
                committer_email=ce,
                committed_at=_date(cd),
                trailers=parse_trailers(trailers),
                message=body.rstrip("\n"),
            )
        )
    return commits


def parse_diff_tree(output: str) -> dict[str, CommitDiff]:
    """Parse ``git diff-tree --stdin -z -r -M -C --root --raw --numstat`` output."""
    tokens = output.split("\x00")
    result: dict[str, CommitDiff] = {}
    current: CommitDiff | None = None
    by_path: dict[str, FileChange] = {}
    i = 0
    while i < len(tokens):
        token = tokens[i]
        i += 1
        if not token:
            continue
        if token.startswith(":"):
            if current is None:
                continue
            fields = token[1:].split(" ")
            status_field = fields[-1] if fields else "M"
            status = status_field[0]
            similarity = (
                int(status_field[1:]) if len(status_field) > 1 and status_field[1:].isdigit() else None
            )
            if status in ("R", "C"):
                old_path, new_path = tokens[i], tokens[i + 1]
                i += 2
                change = FileChange(path=new_path, old_path=old_path, status=status, similarity=similarity)
            else:
                path = tokens[i]
                i += 1
                change = FileChange(path=path, old_path=None, status=status)
            current.files.append(change)
            by_path[change.path] = change
            continue
        match = _NUMSTAT.match(token)
        if match and current is not None:
            added, deleted, path = match.groups()
            if path == "":
                path = tokens[i + 1]  # rename/copy: old then new path follow
                i += 2
            change = by_path.get(path)
            if change is None:
                change = FileChange(path=path, old_path=None, status="M")
                current.files.append(change)
                by_path[path] = change
            if added == "-" or deleted == "-":
                change.binary = True
            else:
                change.insertions = int(added)
                change.deletions = int(deleted)
            continue
        if _HEX.match(token):
            current = CommitDiff(sha=token)
            result[token] = current
            by_path = {}
    return result


def parse_numstat_z(output: str) -> list[FileChange]:
    """Parse ``git diff --numstat -z -M`` (used for base..final change surfaces)."""
    tokens = output.split("\x00")
    files: list[FileChange] = []
    i = 0
    while i < len(tokens):
        token = tokens[i]
        i += 1
        match = _NUMSTAT.match(token) if token else None
        if not match:
            continue
        added, deleted, path = match.groups()
        old_path = None
        status = "M"
        if path == "":
            old_path, path = tokens[i], tokens[i + 1]
            i += 2
            status = "R"
        binary = added == "-" or deleted == "-"
        files.append(
            FileChange(
                path=path,
                old_path=old_path,
                status=status,
                insertions=0 if binary else int(added),
                deletions=0 if binary else int(deleted),
                binary=binary,
            )
        )
    return files


def parse_patch_ids(output: str) -> dict[str, str]:
    """``git patch-id --stable`` output: ``<patch-id> <commit>`` per line -> {commit: patch_id}."""
    ids: dict[str, str] = {}
    for line in output.splitlines():
        parts = line.split()
        if len(parts) == 2:
            ids[parts[1]] = parts[0]
    return ids
