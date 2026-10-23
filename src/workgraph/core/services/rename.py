"""Rename a term (a language, a product) across a project's live records.

A repository renamed its language: the code moved in one commit, and the tasks, tags, project,
repository and verification checks that describe it still say the old name. This module rewrites
those, and only those that are still live:

* **Open tasks** have their title, description, acceptance criteria and file paths rewritten.
* **Closed tasks** (accepted, cancelled or archived) are records of what happened, in the words
  used then. Their text is left exactly as it is. Their **tags** are renamed, so a filter on the
  new tag still finds the whole project's history.
* **The project** (slug, name, description) and **its repositories** (name, description) are
  renamed. A checkout's path on disk is not: moving a directory is not a record change.
* **Verification configs** scoped to the project or its repositories have every string in their
  checks rewritten (commands, environment keys and values, the repository a check runs in). A
  rewritten absolute path that does not exist is reported, because a check that names a file
  nobody renamed will fail when it runs.

Matching is the word, never a fragment of one, in three spellings (``rho``, ``Rho``, ``RHO``): a
spelling is renamed when no letter precedes it and no letter of its own case follows it, so
``neighbourhood`` and ``rhombus`` are left alone while ``rho_kernel``, ``RhoKernel``,
``RHO_VULKAN_TIER`` and ``.rho`` move. ``\\n`` and ``\\t`` escapes count as boundaries. Tokens
passed as ``keep`` (file magics, recorded names) are never touched.

Every change goes through the services, so it is an ordinary event in each record's history.
Text rewrites do not queue reclassification or re-embedding: the words changed, not the task.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from workgraph.core.context import RequestContext
from workgraph.core.errors import ValidationFailed, WorkgraphError
from workgraph.core.models.enums import TaskStatus
from workgraph.core.models.schemas import (
    ProjectUpdate,
    RepositoryUpdate,
    TaskFileIn,
    TaskFilter,
    TaskOut,
    TaskUpdate,
)

CLOSED = frozenset({TaskStatus.ACCEPTED.value, TaskStatus.CANCELLED.value})
PAGE = 500

Report = dict[str, Any]


@dataclass
class Term:
    """The old and new spelling of one word, and the tokens that contain it but are not it."""

    old: str
    new: str
    keep: tuple[str, ...] = ()
    _patterns: list[tuple[re.Pattern[str], str]] = field(init=False, repr=False)

    def __post_init__(self) -> None:
        for word in (self.old, self.new):
            if not word.isascii() or not word.isalpha():
                raise ValidationFailed(f"{word!r} is not a name: one ASCII word, letters only")
        if self.old.lower() == self.new.lower():
            raise ValidationFailed("the old and new names are the same word")
        before = r"(?:(?<![A-Za-z])|(?<=\\[nt]))"
        spellings = [
            (self.old.lower(), self.new.lower(), "(?![a-z])"),
            (_title(self.old), _title(self.new), "(?![a-z])"),
            (self.old.upper(), self.new.upper(), "(?![A-Za-z])"),
        ]
        self._patterns = [(re.compile(before + re.escape(o) + after), n) for o, n, after in spellings]

    def apply(self, text: str) -> tuple[str, int]:
        """Return ``text`` with the term renamed, and how many sites changed."""
        masked = text
        for i, token in enumerate(self.keep):
            masked = masked.replace(token, f"\x00{i}\x00")
        total = 0
        for pattern, replacement in self._patterns:
            masked, count = pattern.subn(replacement, masked)
            total += count
        for i, token in enumerate(self.keep):
            masked = masked.replace(f"\x00{i}\x00", token)
        return masked, total

    def deep(self, value: Any) -> tuple[Any, int]:
        """Rename inside a JSON value: every string, and every object key."""
        if isinstance(value, str):
            return self.apply(value)
        if isinstance(value, list):
            items = [self.deep(v) for v in value]
            return [v for v, _ in items], sum(n for _, n in items)
        if isinstance(value, dict):
            out: dict[str, Any] = {}
            total = 0
            for key, v in value.items():
                new_key, a = self.apply(key) if isinstance(key, str) else (key, 0)
                new_v, b = self.deep(v)
                out[new_key] = new_v
                total += a + b
            return out, total
        return value, 0


def _title(word: str) -> str:
    return word[:1].upper() + word[1:].lower()


def rename_term(
    app: Any,
    ctx: RequestContext,
    workspace: str,
    project: str,
    term: Term,
    *,
    dry_run: bool = False,
) -> Report:
    """Rename ``term`` across ``project``. With ``dry_run`` nothing changes and the report is the same."""
    proj = app.projects.get(ctx, workspace, project)
    repos = [app.repositories.get(ctx, workspace, rid) for rid in proj.repository_ids]
    tasks = _project_tasks(app, ctx, workspace, proj.id)

    report: Report = {
        "from": term.old,
        "to": term.new,
        "dry_run": dry_run,
        "project": None,
        "repositories": [],
        "verification_configs": [],
        "tasks": [],
        "tags_renamed": 0,
        "closed_kept": 0,
        "errors": [],
    }

    def attempt(what: str, action: Any) -> bool:
        if dry_run:
            return True
        try:
            action()
        except WorkgraphError as exc:
            report["errors"].append({"target": what, "error": exc.message})
            return False
        return True

    # Repositories first: a verification check names the repository it runs in, and saving the
    # check resolves that name.
    for repo in repos:
        name, a = term.apply(repo.name)
        description, b = term.apply(repo.description or "")
        if a + b == 0:
            continue
        update = RepositoryUpdate(name=name if a else None, description=description if b else None)
        report["repositories"].append(
            {"name": repo.name, "new_name": name, "description_sites": b, "checkouts_unchanged": True}
        )
        attempt(
            f"repository {repo.name}",
            lambda r=repo, u=update: app.repositories.update(ctx, workspace, r.id, u),
        )

    slug, a = term.apply(proj.slug)
    name, b = term.apply(proj.name)
    description, c = term.apply(proj.description or "")
    if a + b + c:
        report["project"] = {"slug": proj.slug, "new_slug": slug, "name": proj.name, "new_name": name}
        update = ProjectUpdate(
            slug=slug if a else None, name=name if b else None, description=description if c else None
        )
        attempt(f"project {proj.slug}", lambda: app.projects.update(ctx, workspace, proj.id, update))

    scopes = {("project", proj.id), *(("repository", r.id) for r in repos)}
    for config in app.verification.list_configs(ctx, workspace):
        if (config.scope_type, config.scope_id) not in scopes:
            continue
        checks, sites = term.deep(config.checks)
        if not sites:
            continue
        missing = sorted(_missing_paths(config.checks, checks))
        report["verification_configs"].append({"name": config.name, "sites": sites, "missing_paths": missing})
        attempt(
            f"verification config {config.name}",
            lambda cfg=config, ch=checks: app.verification.create_config(
                ctx, workspace, scope_type=cfg.scope_type, scope=cfg.scope_id, name=cfg.name, checks=ch
            ),
        )

    for task in tasks:
        closed = task.status in CLOSED or task.archived_at is not None
        tags = [term.apply(t)[0] for t in task.tags]
        tag_sites = sum(t != o for t, o in zip(tags, task.tags, strict=True))
        changed: dict[str, int] = {}
        update: dict[str, Any] = {}
        if tag_sites:
            update["tags"] = tags
            changed["tags"] = tag_sites
        if closed:
            mentions = sum(term.apply(text)[1] for text in _texts(task))
            report["closed_kept"] += bool(mentions)
        else:
            title, n = term.apply(task.title)
            if n:
                update["title"], changed["title"] = title, n
            description, n = term.apply(task.description or "")
            if n:
                update["description"], changed["description"] = description, n
            criteria = [term.apply(c) for c in task.acceptance_criteria]
            if any(n for _, n in criteria):
                update["acceptance_criteria"] = [c for c, _ in criteria]
                changed["acceptance_criteria"] = sum(n for _, n in criteria)
            files = [(f, term.apply(f.path), term.apply(f.anchor or "")) for f in task.files]
            if any(p[1] or a[1] for _, p, a in files):
                update["files"] = [
                    TaskFileIn(path=p[0], anchor=a[0] or None, note=f.note) for f, p, a in files
                ]
                changed["files"] = sum(p[1] + a[1] for _, p, a in files)
        if not update:
            continue
        report["tags_renamed"] += tag_sites
        report["tasks"].append({"key": task.key, "status": task.status, "closed": closed, "fields": changed})
        data = TaskUpdate(expected_version=task.version, reason=f"rename {term.old} to {term.new}", **update)
        attempt(task.key, lambda t=task, d=data: app.tasks.update(ctx, workspace, t.id, d, enrich=False))
    return report


def _project_tasks(app: Any, ctx: RequestContext, workspace: str, project_id: str) -> list[TaskOut]:
    """Every task of the project, archived ones included."""
    found: list[TaskOut] = []
    offset = 0
    while True:
        page = app.tasks.list(
            ctx,
            workspace,
            TaskFilter(project=project_id, include_archived=True, sort="key", limit=PAGE, offset=offset),
        )
        found.extend(page.items)
        offset += PAGE
        if offset >= page.total or not page.items:
            return found


def _texts(task: TaskOut) -> list[str]:
    return [
        task.title,
        task.description or "",
        *task.acceptance_criteria,
        *(f.path for f in task.files),
        *(f.anchor or "" for f in task.files),
    ]


def _missing_paths(before: Any, after: Any) -> set[str]:
    """Absolute paths the rewrite changed whose new spelling names nothing on disk."""
    out: set[str] = set()
    if isinstance(before, str) and isinstance(after, str):
        if (
            before != after
            and Path(after).is_absolute()
            and Path(before).exists()
            and not Path(after).exists()
        ):
            out.add(after)
    elif isinstance(before, list) and isinstance(after, list):
        for b, a in zip(before, after, strict=True):
            out |= _missing_paths(b, a)
    elif isinstance(before, dict) and isinstance(after, dict):
        for b, a in zip(before.values(), after.values(), strict=True):
            out |= _missing_paths(b, a)
    return out
