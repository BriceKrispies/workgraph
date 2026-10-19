"""Apply a file of triage verdicts to backlog tasks.

A triage run reads old tasks against the code and writes one JSON object per task. This module
turns those verdicts into task changes. Every change goes through the TaskService methods; no ORM
fields are written here.

The whole file is validated before anything changes, so a dry run and a real run report the same
errors. Each service call commits on its own, so an element that fails part way through keeps the
steps that already succeeded; that element is reported as an error.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConflictError,
    InvalidTransitionError,
    NotFoundError,
    PermissionDenied,
    ValidationFailed,
)
from workgraph.core.models.enums import RelationType, TaskPriority, TaskStatus
from workgraph.core.models.schemas import (
    CommentCreate,
    RelationCreate,
    TaskCreate,
    TaskOut,
    TaskUpdate,
)
from workgraph.core.policies.lifecycle import check_task_transition
from workgraph.core.services.tasks import TaskService

VERDICTS = frozenset({"resolved", "obsolete", "open", "needs-decision", "needs-run", "unclear"})
# What a rewrite touches, never an effort estimate. Files written before the scope vocabulary may
# still say small/medium/large; those are read as the scope they always meant.
SIZES = frozenset({"one-file", "one-crate", "several-crates"})
LEGACY_SIZES = {"small": "one-file", "medium": "one-crate", "large": "several-crates"}
TRIAGE_TAG_PREFIX = "triage:"
KIND_TAG_PREFIX = "kind:"
DROPPED_TAG = "wg136"

# Verdicts that move the task to cancelled.
CANCEL_VERDICTS = frozenset({"resolved", "obsolete"})
# Verdicts that leave the task waiting: a later file (a decision made, a run done) may replace them.
PENDING_VERDICTS = frozenset({"needs-decision", "needs-run", "unclear"})

Report = dict[str, list[dict[str, Any]]]


def load_verdicts(path: Path) -> list[Any]:
    """Read a triage file; the top level must be a JSON array."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValidationFailed(f"cannot read triage file {path}: {exc}") from exc
    if not isinstance(data, list):
        raise ValidationFailed("triage file must contain a JSON array of verdicts")
    return data


def apply_verdicts(
    tasks: TaskService,
    ctx: RequestContext,
    workspace: str | None,
    entries: list[Any],
    *,
    dry_run: bool = False,
    source: str | None = None,
) -> Report:
    """Validate every verdict, then apply them one by one unless ``dry_run`` is set.

    Returns ``{"applied": [...], "skipped": [...], "errors": [...]}``.
    """
    planned, report = _plan(tasks, ctx, workspace, entries)
    for item in planned:
        if dry_run:
            report["applied"].append(_applied(item, _action(item.verdict), []))
            continue
        try:
            action, new_keys = _apply_one(tasks, ctx, item, source)
        except (
            ConflictError,
            InvalidTransitionError,
            NotFoundError,
            PermissionDenied,
            ValidationFailed,
        ) as exc:
            report["errors"].append(_error(item.index, item.key, item.verdict, exc.message))
            continue
        report["applied"].append(_applied(item, action, new_keys))
    return report


class _Planned:
    """A verdict that passed validation and resolved to a task."""

    def __init__(self, index: int, entry: dict[str, Any], task: TaskOut) -> None:
        self.index = index
        self.entry = entry
        self.task = task
        self.key: str = task.key
        self.verdict: str = entry["verdict"]


def _plan(
    tasks: TaskService, ctx: RequestContext, workspace: str | None, entries: list[Any]
) -> tuple[list[_Planned], Report]:
    planned: list[_Planned] = []
    report: Report = {"applied": [], "skipped": [], "errors": []}
    seen: set[str] = set()
    for index, entry in enumerate(entries):
        key = entry.get("key") if isinstance(entry, dict) else None
        verdict = entry.get("verdict") if isinstance(entry, dict) else None
        problem = _shape_problem(entry)
        if problem is None and key.strip().upper() in seen:
            problem = f"duplicate key {key} in file"
        if problem is not None:
            report["errors"].append(_error(index, key, verdict, problem))
            continue
        seen.add(key.strip().upper())
        try:
            task = tasks.get(ctx, workspace, key)
        except (NotFoundError, ValidationFailed) as exc:
            report["errors"].append(_error(index, key, verdict, exc.message))
            continue
        existing = [t for t in task.tags if t.startswith(TRIAGE_TAG_PREFIX)]
        pending = [t.removeprefix(TRIAGE_TAG_PREFIX) for t in existing]
        if existing and (verdict in pending or not set(pending) <= PENDING_VERDICTS):
            report["skipped"].append(
                {"key": task.key, "verdict": verdict, "reason": f"already triaged ({existing[0]})"}
            )
            continue
        if verdict in CANCEL_VERDICTS or verdict == "open":
            try:
                check_task_transition(TaskStatus(task.status), TaskStatus.CANCELLED)
            except InvalidTransitionError:
                report["errors"].append(
                    _error(index, key, verdict, f"cannot move {task.key} from {task.status} to cancelled")
                )
                continue
        planned.append(_Planned(index, entry, task))
    return planned, report


def _shape_problem(entry: Any) -> str | None:
    """Return why an element is malformed, or None when it is well formed."""
    if not isinstance(entry, dict):
        return "verdict is not a JSON object"
    key = entry.get("key")
    if not _text(key):
        return "missing key"
    verdict = entry.get("verdict")
    if verdict not in VERDICTS:
        return f"unknown verdict {verdict!r}"
    if "evidence" in entry and not _str_list(entry["evidence"]):
        return "evidence must be a list of strings"
    if verdict == "open":
        rewrites = entry.get("rewrites", [])
        duplicates = entry.get("duplicate_of", [])
        if not isinstance(rewrites, list) or not _str_list(duplicates):
            return "rewrites must be a list and duplicate_of a list of task keys"
        if not rewrites and not duplicates:
            return "open verdict needs at least one rewrite or a duplicate_of task"
        for i, rewrite in enumerate(rewrites):
            if not isinstance(rewrite, dict):
                return f"rewrite {i} is not an object"
            if not _text(rewrite.get("title")):
                return f"rewrite {i} is missing a title"
            if not _text(rewrite.get("description")):
                return f"rewrite {i} is missing a description"
            size = rewrite.get("size")
            if size is not None and LEGACY_SIZES.get(size, size) not in SIZES:
                return f"rewrite {i} has unknown size {size!r}"
            if not _str_list(rewrite.get("files", [])) or not _str_list(
                rewrite.get("acceptance_criteria", [])
            ):
                return f"rewrite {i} files and acceptance_criteria must be lists of strings"
    if verdict == "needs-run" and not _text(entry.get("command")):
        return "needs-run verdict needs a command"
    return None


def _text(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _str_list(value: Any) -> bool:
    return isinstance(value, list) and all(isinstance(v, str) for v in value)


def _action(verdict: str) -> str:
    if verdict in CANCEL_VERDICTS:
        return "cancelled"
    if verdict == "open":
        return "superseded"
    return "commented"


def _applied(item: _Planned, action: str, new_keys: list[str]) -> dict[str, Any]:
    return {"key": item.key, "verdict": item.verdict, "action": action, "new_keys": new_keys, "error": None}


def _error(index: int, key: Any, verdict: Any, message: str) -> dict[str, Any]:
    return {"index": index, "key": key, "verdict": verdict, "error": message}


def _head(source: str | None) -> str:
    return f"Triage ({source}): " if source else "Triage: "


def _body(source: str | None, lead: str, summary: str, evidence: list[str], blocks: list[str] = ()) -> str:
    """Build a triage comment: lead line, extra blocks, then the evidence list."""
    text = f"{_head(source)}{lead}. {summary}".rstrip()
    for block in blocks:
        text += f"\n\n{block}"
    if evidence:
        text += "\n\nEvidence:\n" + "\n".join(f"- {e}" for e in evidence)
    return text


def _apply_one(
    tasks: TaskService, ctx: RequestContext, item: _Planned, source: str | None
) -> tuple[str, list[str]]:
    entry = item.entry
    summary = str(entry.get("summary") or "")
    evidence = list(entry.get("evidence") or [])
    ws_id = item.task.workspace_id
    key = item.key
    verdict = item.verdict
    # A pending verdict being replaced: drop its tags so the task carries only the new one.
    stale = [t for t in item.task.tags if t.startswith(TRIAGE_TAG_PREFIX) or t == "needs-decision"]
    if stale:
        tasks.update(ctx, ws_id, key, TaskUpdate(remove_tags=stale))

    if verdict in CANCEL_VERDICTS:
        _comment(tasks, ctx, ws_id, key, _body(source, verdict, summary, evidence))
        _tag(tasks, ctx, ws_id, key, [f"{TRIAGE_TAG_PREFIX}{verdict}"])
        _cancel(tasks, ctx, ws_id, key, f"triage: {verdict}")
        return "cancelled", []

    if verdict == "open":
        new_keys = [_create_follow_up(tasks, ctx, item, rewrite) for rewrite in entry.get("rewrites", [])]
        # Open work an existing task already tracks: the original duplicates that task, nothing is created.
        duplicates = list(entry.get("duplicate_of", []))
        for target in duplicates:
            tasks.add_relation(
                ctx, ws_id, key, RelationCreate(target=target, relation_type=RelationType.DUPLICATES)
            )
        keys = ", ".join(new_keys + duplicates)
        _comment(tasks, ctx, ws_id, key, _body(source, f"superseded by {keys}", summary, evidence))
        _tag(tasks, ctx, ws_id, key, [f"{TRIAGE_TAG_PREFIX}superseded"])
        _cancel(tasks, ctx, ws_id, key, f"triage: superseded by {keys}")
        return "superseded", new_keys

    if verdict == "needs-decision":
        decision = entry.get("decision")
        blocks = [f"Decision: {decision}"] if decision else []
        _comment(tasks, ctx, ws_id, key, _body(source, "needs a decision", summary, evidence, blocks))
        _tag(tasks, ctx, ws_id, key, ["needs-decision", f"{TRIAGE_TAG_PREFIX}needs-decision"])
        return "commented", []

    if verdict == "needs-run":
        blocks = [f"Confirm with: {entry['command']}"]
        _comment(tasks, ctx, ws_id, key, _body(source, "needs a run to confirm", summary, evidence, blocks))
        _tag(tasks, ctx, ws_id, key, [f"{TRIAGE_TAG_PREFIX}needs-run"])
        return "commented", []

    # unclear
    _comment(tasks, ctx, ws_id, key, _body(source, "unclear", summary, evidence))
    _tag(tasks, ctx, ws_id, key, [f"{TRIAGE_TAG_PREFIX}unclear"])
    return "commented", []


def _create_follow_up(
    tasks: TaskService, ctx: RequestContext, item: _Planned, rewrite: dict[str, Any]
) -> str:
    original = item.task
    summary = str(item.entry.get("summary") or "")
    files = list(rewrite.get("files") or [])
    size = rewrite.get("size")

    description = rewrite["description"]
    if files:
        description += f"\n\n**Files:** {', '.join(files)}"
    description += f"\n\nTriaged from {original.key}: {summary}"

    tags = [t for t in original.tags if t != DROPPED_TAG and not t.startswith(KIND_TAG_PREFIX)]
    tags.append("triaged")
    if size:
        tags.append(f"scope:{LEGACY_SIZES.get(size, size)}")

    created = tasks.create(
        ctx,
        original.workspace_id,
        TaskCreate(
            title=rewrite["title"],
            description=description,
            acceptance_criteria=list(rewrite.get("acceptance_criteria") or []),
            priority=TaskPriority(original.priority),
            tags=list(dict.fromkeys(tags)),
            projects=list(original.project_ids),
            repositories=list(original.repository_ids),
        ),
    )
    tasks.add_relation(
        ctx,
        original.workspace_id,
        created.key,
        RelationCreate(target=original.key, relation_type=RelationType.FOLLOWS_UP),
    )
    return created.key


def _comment(tasks: TaskService, ctx: RequestContext, ws_id: str, key: str, body: str) -> None:
    tasks.add_comment(ctx, ws_id, key, CommentCreate(body=body))


def _tag(tasks: TaskService, ctx: RequestContext, ws_id: str, key: str, tags: list[str]) -> None:
    tasks.update(ctx, ws_id, key, TaskUpdate(add_tags=tags))


def _cancel(tasks: TaskService, ctx: RequestContext, ws_id: str, key: str, reason: str) -> None:
    tasks.update(ctx, ws_id, key, TaskUpdate(status=TaskStatus.CANCELLED, reason=reason))
