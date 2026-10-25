"""Import a task breakdown written as a markdown section.

A planning document lists its tasks as bold headers at the start of a line (``**V1. Title**``,
``**FD-01 Title**``). A ranged header (``**GV-01 to GV-21 Name**``) is followed by a markdown table
whose first column holds one id per task; each row becomes one task titled ``Name: <second column>``.
The text under a header, up to the next header, is the description. A ``Depends on:`` or ``Deps:``
list names other ids (ranges such as ``SC-01 to SC-36`` expand) or existing task keys. Anything else
in such a list is reported as an unresolved dependency and stays in the description. ``Tier: mid``
and ``Tier: top`` become the tags ``tier-mid`` and ``tier-top``.

Every created task carries ``metadata = {"breakdown_id": ID, "breakdown_doc": "<source>#<section>"}``.
A re-run updates the tasks it finds with that metadata instead of creating them again.

The markdown is validated before anything changes. Each service call commits on its own, as for
triage verdicts. A dry run plans the same actions and changes nothing.
"""

from __future__ import annotations

import re
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field
from typing import Any

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.models.enums import TaskStatus
from workgraph.core.models.schemas import TaskCreate, TaskFilter, TaskOut, TaskUpdate
from workgraph.core.services.tasks import TaskService

ID = r"[A-Z][A-Z]*-?\d+"
ID_RE = re.compile(rf"^{ID}$")
RANGE_RE = re.compile(r"^([A-Z][A-Z]*-?)(\d+)\s+to\s+([A-Z][A-Z]*-?)(\d+)$")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*$")
HEADER_RE = re.compile(rf"^\*\*({ID})\.?(?:\s+to\s+({ID}))?\s+(.+?)\*\*(.*)$")
DEPS_RE = re.compile(r"(?i)\b(?:depends on|deps)\s*:\s*(.*)$")
TIER_RE = re.compile(r"(?i)\bTier\s*:\s*(mid|top)\b")
TIER_CUT_RE = re.compile(r"(?i)\bTier\s*:")
PAREN_RE = re.compile(r"^(.*?)\s*\((.*)\)\s*$")
SEPARATOR_RE = re.compile(r"\s*(?:[,;]|\.(?=\s|$)|\band\b)\s*")

TIER_TAGS = ("tier-mid", "tier-top")
NO_DEPS = frozenset({"none", "-", "—"})  # "none" and dashes in a Deps cell


@dataclass
class Item:
    """One task of the breakdown, before any write."""

    id: str
    title: str
    description: str
    tier_tags: list[str] = field(default_factory=list)
    deps: list[str] = field(default_factory=list)  # ids, or free text that is reported unresolved
    block: str | None = None  # the ranged header a table row belongs to


def _norm(text: str) -> str:
    return " ".join(text.replace("**", " ").replace("#", " ").split()).lower()


def _fence_toggles(lines: list[str]) -> Iterator[tuple[int, str, bool]]:
    """Yield (index, line, inside_fence) so that headings inside code fences are ignored."""
    fenced = False
    for index, line in enumerate(lines):
        if line.lstrip().startswith("```"):
            fenced = not fenced
            yield index, line, True
            continue
        yield index, line, fenced


def section_lines(markdown: str, section: str) -> list[str]:
    """The lines under the heading whose text is ``section``, up to the next heading of the same or a higher level."""
    lines = markdown.splitlines()
    wanted = _norm(section)
    headings: list[tuple[int, int, str]] = []
    for index, line, fenced in _fence_toggles(lines):
        match = None if fenced else HEADING_RE.match(line)
        if match:
            headings.append((index, len(match.group(1)), match.group(2)))
    found = [(index, level) for index, level, text in headings if _norm(text) == wanted]
    if not found:
        raise ValidationFailed(f"no heading {section!r} in the document")
    if len(found) > 1:
        raise ValidationFailed(f"the heading {section!r} occurs {len(found)} times; rename one of them")
    start, level = found[0]
    end = next((index for index, other, _ in headings if index > start and other <= level), len(lines))
    return lines[start + 1 : end]


def _expand(token: str) -> list[str] | None:
    """Expand ``SC-01 to SC-36``; None when the text is not a range of ids."""
    match = RANGE_RE.match(token)
    if match is None or match.group(1) != match.group(3):
        return None
    first, last = int(match.group(2)), int(match.group(4))
    if last < first:
        return None
    width = len(match.group(2))
    return [f"{match.group(1)}{number:0{width}d}" for number in range(first, last + 1)]


def _dep_tokens(rest: str) -> list[str]:
    """Split the text after ``Depends on:`` into ids and free text (a parenthetical is its own token)."""
    rest = TIER_CUT_RE.split(rest, maxsplit=1)[0]
    tokens: list[str] = []
    for piece in SEPARATOR_RE.split(rest):
        piece = piece.strip().rstrip(".").strip()
        if not piece or piece.lower() in NO_DEPS:
            continue
        paren = PAREN_RE.match(piece)
        for part in [paren.group(1), paren.group(2)] if paren else [piece]:
            part = part.strip().rstrip(".").strip()
            if not part:
                continue
            expanded = _expand(part)
            tokens.extend(expanded if expanded is not None else [part])
    return tokens


def _joined(lines: Iterable[str]) -> Iterator[str]:
    """Join a line that opens a parenthesis with the following lines until it closes."""
    pending = ""
    for line in lines:
        pending = f"{pending} {line}".strip() if pending else line
        if pending.count("(") <= pending.count(")"):
            yield pending
            pending = ""
    if pending:
        yield pending


def _text_fields(lines: Iterable[str]) -> tuple[list[str], list[str]]:
    """The dependency tokens and the tier tags named by the given lines."""
    deps: list[str] = []
    tags: list[str] = []
    for line in _joined(lines):
        match = DEPS_RE.search(line)
        if match:
            for token in _dep_tokens(match.group(1)):
                if token not in deps:
                    deps.append(token)
        for tier in TIER_RE.finditer(line):
            tag = f"tier-{tier.group(1).lower()}"
            if tag not in tags:
                tags.append(tag)
    return deps, tags


def _cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def _table_columns(table: list[str]) -> dict[str, int]:
    """Column index by lower-cased header name; the header is the first row that is neither an id nor a rule."""
    for line in table:
        cells = _cells(line)
        if cells and not ID_RE.match(cells[0]) and not set(cells[0]) <= set("-: "):
            return {cell.lower(): index for index, cell in enumerate(cells)}
    return {}


def _row_fields(cells: list[str], columns: dict[str, int]) -> tuple[list[str], list[str]]:
    deps: list[str] = []
    for name in ("deps", "depends on"):
        if name in columns and columns[name] < len(cells):
            deps.extend(_dep_tokens(cells[columns[name]]))
    tiers: list[str] = []
    index = columns.get("tier")
    if index is not None and index < len(cells) and cells[index].lower() in ("mid", "top"):
        tiers.append(f"tier-{cells[index].lower()}")
    return deps, tiers


def _blocks(lines: list[str]) -> list[tuple[str, str | None, str, list[str]]]:
    """Group the section's lines under their bold headers: (id, range end, name, body lines)."""
    blocks: list[tuple[str, str | None, str, list[str]]] = []
    for _index, line, fenced in _fence_toggles(lines):
        header = None if fenced else HEADER_RE.match(line)
        if header:
            # Text after the closing ** of a header (e.g. "(each: ...)") opens the block's body.
            trailing = header.group(4).strip()
            blocks.append((header.group(1), header.group(2), header.group(3), [trailing] if trailing else []))
        elif blocks:
            blocks[-1][3].append(line)
    return blocks


def parse_items(lines: list[str]) -> list[Item]:
    """Turn the lines of one section into the tasks it describes, in document order."""
    items: list[Item] = []
    for block_id, range_end, name, body in _blocks(lines):
        if range_end is None:
            deps, tiers = _text_fields(body)
            items.append(
                Item(
                    id=block_id,
                    title=name,
                    description="\n".join(body).strip(),
                    tier_tags=tiers,
                    deps=deps,
                )
            )
            continue
        table = [line for line in body if line.lstrip().startswith("|")]
        prose = "\n".join(line for line in body if not line.lstrip().startswith("|")).strip()
        deps, tiers = _text_fields(prose.splitlines())
        columns = _table_columns(table)
        rows = [(line, _cells(line)) for line in table]
        rows = [(line, cells) for line, cells in rows if ID_RE.match(cells[0])]
        if not rows:
            raise ValidationFailed(f"ranged block {block_id} to {range_end} ({name}) has no table rows")
        for line, cells in rows:
            row_name = cells[1] if len(cells) > 1 and cells[1] else ""
            # A "Deps" or "Tier" column, when the table has one, applies to its row only.
            row_deps, row_tiers = _row_fields(cells, columns)
            items.append(
                Item(
                    id=cells[0],
                    title=f"{name}: {row_name}" if row_name else name,
                    description=(prose + "\n\n" + line.strip()).strip(),
                    tier_tags=_unique([*tiers, *row_tiers]),
                    deps=_unique([*deps, *row_deps]),
                    block=block_id,
                )
            )
    seen: set[str] = set()
    for item in items:
        if item.id in seen:
            raise ValidationFailed(f"breakdown id {item.id} appears more than once in the section")
        seen.add(item.id)
    return items


@dataclass
class _Plan:
    item: Item
    current: TaskOut | None
    action: str  # created, updated or unchanged
    desired_tags: list[str]
    changes: dict[str, Any]
    add_tags: list[str]
    remove_tags: list[str]
    deps_to_add: list[tuple[str, Any]]
    ready: bool
    status: str  # the status after the import
    key: str | None = None


def _unique(values: Iterable[str]) -> list[str]:
    out: list[str] = []
    for value in values:
        if value not in out:
            out.append(value)
    return out


def _existing_by_id(
    tasks: TaskService, ctx: RequestContext, workspace: str | None, source: str
) -> dict[str, TaskOut]:
    """Tasks already imported from ``source`` (a ``<path>#<section>`` string), by breakdown id."""
    found: dict[str, TaskOut] = {}
    offset = 0
    while True:
        page = tasks.list(ctx, workspace, TaskFilter(include_archived=True, limit=500, offset=offset))
        for task in page.items:
            if task.metadata.get("breakdown_doc") == source:
                found.setdefault(str(task.metadata.get("breakdown_id")), task)
        if page.next_offset is None:
            return found
        offset = page.next_offset


def _lookup_key(tasks: TaskService, ctx: RequestContext, workspace: str | None, token: str) -> TaskOut | None:
    try:
        return tasks.get(ctx, workspace, token)
    except (NotFoundError, ValidationFailed):
        return None


def import_breakdown(
    tasks: TaskService,
    ctx: RequestContext,
    workspace: str | None,
    markdown: str,
    *,
    section: str,
    source: str,
    parent: str | None = None,
    tags: Iterable[str] = (),
    project: str | None = None,
    repository: str | None = None,
    dry_run: bool = False,
) -> dict[str, Any]:
    """Create or update the tasks of ``section``; ``source`` names the document in the metadata."""
    items = parse_items(section_lines(markdown, section))
    if not items:
        raise ValidationFailed(f"the section {section!r} holds no tasks")
    doc_ref = f"{source}#{section}"
    ids = {item.id for item in items}
    existing = _existing_by_id(tasks, ctx, workspace, doc_ref)
    parent_task = tasks.get(ctx, workspace, parent) if parent else None
    extra_tags = list(tags)

    unresolved: list[dict[str, str]] = []
    refs: dict[str, list[tuple[str, Any]]] = {}
    for item in items:
        found: list[tuple[str, Any]] = []
        for token in item.deps:
            if not ID_RE.match(token):
                unresolved.append({"id": item.id, "dependency": token, "reason": "not a task id"})
            elif token == item.id:
                continue  # a ranged block's shared "Deps:" may name its own range; that is not a dependency
            elif token in ids:
                found.append(("id", token))
            else:
                target = _lookup_key(tasks, ctx, workspace, token)
                if target is None:
                    unresolved.append(
                        {
                            "id": item.id,
                            "dependency": token,
                            "reason": "no such task in this section or the workspace",
                        }
                    )
                else:
                    found.append(("task", target))
        refs[item.id] = found

    def accepted(ref: tuple[str, Any]) -> bool:
        target = existing.get(ref[1]) if ref[0] == "id" else ref[1]
        return target is not None and target.status == TaskStatus.ACCEPTED

    def present(current: TaskOut, ref: tuple[str, Any]) -> bool:
        if ref[0] == "id":
            target = existing.get(ref[1])
            return target is not None and target.id in current.depends_on
        return ref[1].id in current.depends_on

    plans: list[_Plan] = []
    for item in items:
        current = existing.get(item.id)
        desired = _unique([*extra_tags, *item.tier_tags])
        ready = not any(u["id"] == item.id for u in unresolved) and all(accepted(r) for r in refs[item.id])
        changes: dict[str, Any] = {}
        add_tags: list[str] = []
        remove_tags: list[str] = []
        if current is None:
            action = "created"
            status = TaskStatus.READY if ready else TaskStatus.BACKLOG
            deps_to_add = list(refs[item.id])
        else:
            if current.title != item.title:
                changes["title"] = item.title
            if current.description != item.description:
                changes["description"] = item.description
            if parent_task is not None and current.parent_id != parent_task.id:
                changes["parent"] = parent
            add_tags = [t for t in desired if t not in current.tags]
            remove_tags = [t for t in current.tags if t in TIER_TAGS and t not in desired]
            deps_to_add = [r for r in refs[item.id] if not present(current, r)]
            status = TaskStatus(current.status)
            if status == TaskStatus.BACKLOG and ready:
                status = TaskStatus.READY
            changed = bool(changes or add_tags or remove_tags or deps_to_add or status != current.status)
            action = "updated" if changed else "unchanged"
        plans.append(
            _Plan(
                item=item,
                current=current,
                action=action,
                desired_tags=desired,
                changes=changes,
                add_tags=add_tags,
                remove_tags=remove_tags,
                deps_to_add=deps_to_add,
                ready=ready,
                status=status,
                key=current.key if current else None,
            )
        )

    if not dry_run:
        _apply(tasks, ctx, workspace, plans, parent, project, repository, doc_ref)

    keys = {plan.item.id: plan.key for plan in plans}
    rows = [
        {
            "id": plan.item.id,
            "key": plan.key,
            "title": plan.item.title,
            "action": plan.action,
            "status": plan.status,
            "depends_on": [
                (keys.get(ref[1]) or ref[1]) if ref[0] == "id" else ref[1].key for ref in refs[plan.item.id]
            ],
            "tags": plan.desired_tags,
        }
        for plan in plans
    ]
    counts = {name: sum(1 for p in plans if p.action == name) for name in ("created", "updated", "unchanged")}
    counts["ready"] = sum(1 for p in plans if p.status == TaskStatus.READY)
    counts["backlog"] = sum(1 for p in plans if p.status == TaskStatus.BACKLOG)
    return {
        "source": doc_ref,
        "dry_run": dry_run,
        "tasks": rows,
        "map": dict(keys),
        "unresolved": unresolved,
        "counts": counts,
    }


def _apply(
    tasks: TaskService,
    ctx: RequestContext,
    workspace: str | None,
    plans: list[_Plan],
    parent: str | None,
    project: str | None,
    repository: str | None,
    doc_ref: str,
) -> None:
    """Write the plan: create, update, add dependencies, then promote the ready tasks."""
    for plan in plans:
        if plan.current is None:
            out = tasks.create(
                ctx,
                workspace,
                TaskCreate(
                    title=plan.item.title,
                    description=plan.item.description,
                    tags=plan.desired_tags,
                    parent=parent,
                    projects=[project] if project else [],
                    repositories=[repository] if repository else [],
                    metadata={"breakdown_id": plan.item.id, "breakdown_doc": doc_ref},
                ),
            )
            plan.key = out.key
    for plan in plans:
        if plan.current is not None and (plan.changes or plan.add_tags or plan.remove_tags):
            # Only the fields that changed are passed: an explicit None would be written as a value.
            fields: dict[str, Any] = dict(plan.changes)
            if plan.add_tags:
                fields["add_tags"] = plan.add_tags
            if plan.remove_tags:
                fields["remove_tags"] = plan.remove_tags
            tasks.update(ctx, workspace, plan.key or "", TaskUpdate(**fields))
    key_of = {plan.item.id: plan.key for plan in plans}
    for plan in plans:
        for ref in plan.deps_to_add:
            dep_key = key_of.get(ref[1]) if ref[0] == "id" else ref[1].key
            if dep_key is None:
                raise ValidationFailed(f"dependency {ref[1]} of {plan.item.id} has no task")
            tasks.add_dependency(ctx, workspace, plan.key or "", dep_key)
    for plan in plans:
        if plan.status == TaskStatus.READY and (
            plan.current is None or plan.current.status == TaskStatus.BACKLOG
        ):
            tasks.update(ctx, workspace, plan.key or "", TaskUpdate(status=TaskStatus.READY))
