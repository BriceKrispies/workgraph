"""Task commands."""

from __future__ import annotations

from datetime import datetime
from typing import Any, cast

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    fail,
    handle_errors,
    kv,
    parse_json_arg,
    table,
    when,
    workspace_option,
    ws,
)
from workgraph.core.models.enums import RelationType, TaskPriority, TaskStatus
from workgraph.core.models.schemas import (
    BulkUpdate,
    ClaimRequest,
    CommentCreate,
    ProgressReport,
    RelationCreate,
    ResultReport,
    TaskCreate,
    TaskFilter,
    TaskUpdate,
)

task_app = typer.Typer(help="Create, query and move tasks through their lifecycle.", no_args_is_help=True)


def _row(t: Any) -> tuple[Any, ...]:
    return (t.key, t.status, t.priority, t.category or "", t.title[:70], ",".join(t.tags), t.claimed_by or "")


HEAD = ["key", "status", "priority", "category", "title", "tags", "claimed by"]


def _render_task(t: Any) -> None:
    kv(
        [
            ("id", t.id),
            ("title", t.title),
            ("status", t.status),
            ("priority", t.priority),
            ("category", t.category),
            ("tags", ", ".join(t.tags)),
            ("assignee", t.assignee),
            ("claimed by", t.claimed_by),
            ("parent", t.parent_id),
            ("children", len(t.child_ids) or None),
            ("depends on", len(t.depends_on) or None),
            ("blocks", len(t.blocks) or None),
            ("projects", len(t.project_ids) or None),
            ("repositories", len(t.repository_ids) or None),
            ("version", t.version),
            ("created", when(t.created_at)),
            ("completed", when(t.completed_at)),
            ("accepted", when(t.accepted_at)),
        ],
        title=t.key,
    )
    if getattr(t, "description", ""):
        console.print()
        console.print(t.description)
    if t.acceptance_criteria:
        console.print("\n[bold]Acceptance criteria[/bold]")
        for item in t.acceptance_criteria:
            console.print(f"  - {item}")
    rollup = getattr(t, "rollup", None)
    if rollup:
        console.print(
            f"\n[bold]Rollup[/bold]: {rollup.accepted}/{rollup.total_descendants} accepted "
            f"({rollup.progress:.0%}) {rollup.by_status}"
        )
    dep = getattr(t, "dependency_state", None)
    if dep and not dep.satisfied:
        console.print("[yellow]Blocked by:[/yellow] " + ", ".join(b.key for b in dep.unsatisfied))
    for c in getattr(t, "comments", [])[-10:]:
        console.print(f"\n[dim]{when(c.created_at)} {c.author} ({c.kind})[/dim]\n{c.body}")


@task_app.command("create")
@handle_errors
def task_create(
    title: str,
    description: str = typer.Option("", "--description", "-d", help="Markdown description (or @file)"),
    criteria: list[str] = typer.Option(None, "--criterion", "-c", help="Acceptance criterion (repeatable)"),
    priority: TaskPriority = typer.Option(TaskPriority.MEDIUM, "--priority", "-P"),
    ready: bool = typer.Option(False, "--ready", help="Create in the ready state"),
    tag: list[str] = typer.Option(None, "--tag", "-t"),
    project: list[str] = typer.Option(None, "--project", "-p"),
    repo: list[str] = typer.Option(None, "--repo", "-r"),
    parent: str | None = typer.Option(None, "--parent"),
    depends_on: list[str] = typer.Option(None, "--depends-on"),
    assignee: str | None = typer.Option(None),
    agent: str | None = typer.Option(None, help="Preferred agent adapter"),
    auto_execute: bool = typer.Option(False, help="Allow the scheduler to run an agent for this task"),
    due: datetime | None = typer.Option(None, help="Due date (ISO-8601)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Create a task."""
    if description.startswith("@"):
        from pathlib import Path

        description = Path(description[1:]).read_text(encoding="utf-8")
    out = STATE.app().tasks.create(
        STATE.ctx(),
        ws(workspace),
        TaskCreate(
            title=title,
            description=description,
            acceptance_criteria=criteria or [],
            priority=priority,
            status="ready" if ready else "backlog",
            tags=tag or [],
            projects=project or [],
            repositories=repo or [],
            parent=parent,
            depends_on=depends_on or [],
            assignee=assignee,
            assigned_agent=agent,
            auto_execute=auto_execute,
            due_at=due,
        ),
    )
    emit(out, lambda t: console.print(f"created [bold]{t.key}[/bold] ({t.status}) {t.title}"))


@task_app.command("list")
@handle_errors
def task_list(
    status: list[TaskStatus] = typer.Option(None, "--status", "-s"),
    priority: list[TaskPriority] = typer.Option(None, "--priority", "-P"),
    project: str | None = typer.Option(None, "--project", "-p"),
    repo: str | None = typer.Option(None, "--repo", "-r"),
    tag: list[str] = typer.Option(None, "--tag", "-t"),
    category: str | None = typer.Option(None),
    assignee: str | None = typer.Option(None),
    parent: str | None = typer.Option(None),
    roots: bool = typer.Option(False, "--roots", help="Only top-level tasks"),
    search: str | None = typer.Option(None, "--search", "-q", help="Full-text query"),
    archived: bool = typer.Option(False, "--archived", help="Only archived tasks"),
    sort: str = typer.Option("created_at"),
    asc: bool = typer.Option(False, "--asc"),
    limit: int = typer.Option(50),
    offset: int = typer.Option(0),
    workspace: str | None = workspace_option(),
) -> None:
    """List tasks with filters, sorting and pagination."""
    page = STATE.app().tasks.list(
        STATE.ctx(),
        ws(workspace),
        TaskFilter(
            statuses=status or None,
            priorities=priority or None,
            project=project,
            repository=repo,
            tags=tag or None,
            category=category,
            assignee=assignee,
            parent=parent,
            root_only=roots,
            text=search,
            archived_only=archived,
            sort=cast(Any, sort),
            descending=not asc,
            limit=limit,
            offset=offset,
        ),
    )

    def human(p: Any) -> None:
        table([_row(t) for t in p.items], HEAD)
        console.print(
            f"[dim]{len(p.items)} of {p.total}"
            + (f"; next: --offset {p.next_offset}" if p.next_offset is not None else "")
            + "[/dim]"
        )

    emit(page, human)


@task_app.command("ready")
@handle_errors
def task_ready(
    project: str | None = typer.Option(None, "--project", "-p"),
    repo: str | None = typer.Option(None, "--repo", "-r"),
    agent: str | None = typer.Option(None),
    limit: int = typer.Option(20),
    workspace: str | None = workspace_option(),
) -> None:
    """Ready tasks whose dependencies are accepted, highest priority first."""
    items = STATE.app().tasks.get_ready(
        STATE.ctx(), ws(workspace), project=project, repository=repo, agent=agent, limit=limit
    )
    emit(items, lambda rows: table([_row(t) for t in rows], HEAD, title="ready"))


@task_app.command("show")
@handle_errors
def task_show(ref: str, workspace: str | None = workspace_option()) -> None:
    """Show task details, comments, rollup and blockers."""
    emit(STATE.app().tasks.detail(STATE.ctx(), ws(workspace), ref), _render_task)


@task_app.command("update")
@handle_errors
def task_update(
    ref: str,
    title: str | None = typer.Option(None),
    description: str | None = typer.Option(None, "--description", "-d"),
    status: TaskStatus | None = typer.Option(None, "--status", "-s"),
    priority: TaskPriority | None = typer.Option(None, "--priority", "-P"),
    add_tag: list[str] = typer.Option(None, "--add-tag"),
    remove_tag: list[str] = typer.Option(None, "--remove-tag"),
    parent: str | None = typer.Option(None),
    clear_parent: bool = typer.Option(False),
    assignee: str | None = typer.Option(None),
    agent: str | None = typer.Option(None),
    auto_execute: bool | None = typer.Option(None, "--auto-execute/--no-auto-execute"),
    metadata: str | None = typer.Option(None, help="JSON object merged into metadata"),
    expected_version: int | None = typer.Option(None, help="Fail if the task changed since this version"),
    reason: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Update task fields and/or status (validated transitions)."""
    fields: dict[str, Any] = {
        "title": title,
        "description": description,
        "status": status,
        "priority": priority,
        "add_tags": add_tag or None,
        "remove_tags": remove_tag or None,
        "parent": parent,
        "clear_parent": clear_parent,
        "assignee": assignee,
        "assigned_agent": agent,
        "auto_execute": auto_execute,
        "metadata": parse_json_arg(metadata, "metadata"),
        "expected_version": expected_version,
        "reason": reason,
    }
    update = TaskUpdate(**{k: v for k, v in fields.items() if v is not None and v is not False})
    out = STATE.app().tasks.update(STATE.ctx(), ws(workspace), ref, update)
    emit(out, lambda t: console.print(f"{t.key} updated (v{t.version}, {t.status})"))


@task_app.command("bulk-update")
@handle_errors
def task_bulk(
    refs: list[str],
    status: TaskStatus | None = typer.Option(None, "--status", "-s"),
    priority: TaskPriority | None = typer.Option(None, "--priority", "-P"),
    add_tag: list[str] = typer.Option(None, "--add-tag"),
    remove_tag: list[str] = typer.Option(None, "--remove-tag"),
    workspace: str | None = workspace_option(),
) -> None:
    """Apply one change to many tasks."""
    update = TaskUpdate(
        status=status, priority=priority, add_tags=add_tag or None, remove_tags=remove_tag or None
    )
    result = STATE.app().tasks.bulk_update(STATE.ctx(), ws(workspace), BulkUpdate(tasks=refs, update=update))
    emit(
        result, lambda r: console.print(f"updated {len(r.updated)}; failed {len(r.failed)} {r.failed or ''}")
    )
    if result.failed:
        raise typer.Exit(3)


@task_app.command("claim")
@handle_errors
def task_claim(
    ref: str,
    agent: str | None = typer.Option(None),
    lease_minutes: int = typer.Option(120),
    workspace: str | None = workspace_option(),
) -> None:
    """Claim a ready task."""
    out = STATE.app().tasks.claim(
        STATE.ctx(), ws(workspace), ref, ClaimRequest(agent=agent, lease_minutes=lease_minutes)
    )
    emit(out, lambda t: console.print(f"claimed {t.key} until {when(t.claim_expires_at)}"))


@task_app.command("release")
@handle_errors
def task_release(
    ref: str, reason: str | None = typer.Option(None), workspace: str | None = workspace_option()
) -> None:
    """Release a claim."""
    emit(
        STATE.app().tasks.release(STATE.ctx(), ws(workspace), ref, reason),
        lambda t: console.print(f"released {t.key}"),
    )


@task_app.command("complete")
@handle_errors
def task_complete(
    ref: str,
    summary: str = typer.Option("", "--summary", "-m"),
    attempt: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Report completion (moves to awaiting verification/review, never directly to accepted)."""
    out = STATE.app().tasks.complete(STATE.ctx(), ws(workspace), ref, summary=summary, attempt_id=attempt)
    emit(out, lambda t: console.print(f"{t.key} is now {t.status}"))


@task_app.command("accept")
@handle_errors
def task_accept(
    ref: str,
    manual: bool = typer.Option(False, "--manual", help="Record a manual acceptance decision"),
    reason: str = typer.Option("", "--reason", "-m"),
    workspace: str | None = workspace_option(),
) -> None:
    """Accept completed work (requires passing verification, or --manual with a reason)."""
    app = STATE.app()
    if manual:
        out = app.verification.accept_manually(STATE.ctx(), ws(workspace), ref, reason=reason)
    else:
        out = app.verification.accept(STATE.ctx(), ws(workspace), ref, reason=reason)
    emit(out, lambda a: console.print(f"accepted ({a.mode}) by {a.actor}"))


@task_app.command("reject")
@handle_errors
def task_reject(
    ref: str, reason: str = typer.Option(..., "--reason", "-m"), workspace: str | None = workspace_option()
) -> None:
    """Reject completed work and send it back to ready."""
    out = STATE.app().verification.reject(STATE.ctx(), ws(workspace), ref, reason=reason)
    emit(out, lambda a: console.print("rejected"))


@task_app.command("comment")
@handle_errors
def task_comment(ref: str, body: str, workspace: str | None = workspace_option()) -> None:
    """Add a comment."""
    out = STATE.app().tasks.add_comment(STATE.ctx(), ws(workspace), ref, CommentCreate(body=body))
    emit(out, lambda c: console.print("comment added"))


@task_app.command("progress")
@handle_errors
def task_progress(
    ref: str,
    message: str,
    percent: float | None = typer.Option(None),
    attempt: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Report progress on a task."""
    out = STATE.app().tasks.report_progress(
        STATE.ctx(), ws(workspace), ref, ProgressReport(message=message, percent=percent, attempt_id=attempt)
    )
    emit(out, lambda c: console.print("progress recorded"))


@task_app.command("result")
@handle_errors
def task_result(
    ref: str,
    outcome: str = typer.Argument(..., help="succeeded or failed"),
    summary: str = typer.Option("", "--summary", "-m"),
    attempt: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Report an attempt result."""
    if outcome not in ("succeeded", "failed"):
        fail("outcome must be succeeded or failed", 2)
    out = STATE.app().tasks.report_result(
        STATE.ctx(),
        ws(workspace),
        ref,
        ResultReport(outcome=cast(Any, outcome), summary=summary, attempt_id=attempt),
    )
    emit(out, lambda t: console.print(f"{t.key} is now {t.status}"))


@task_app.command("depend")
@handle_errors
def task_depend(
    ref: str,
    depends_on: str,
    remove: bool = typer.Option(False, help="Remove the dependency"),
    workspace: str | None = workspace_option(),
) -> None:
    """Add (or remove) a dependency: REF cannot proceed until DEPENDS_ON is accepted."""
    app = STATE.app()
    if remove:
        out = app.tasks.remove_dependency(STATE.ctx(), ws(workspace), ref, depends_on)
    else:
        out = app.tasks.add_dependency(STATE.ctx(), ws(workspace), ref, depends_on)
    emit(out, lambda t: console.print(f"{t.key} depends on {len(t.depends_on)} task(s); status {t.status}"))


@task_app.command("relate")
@handle_errors
def task_relate(
    ref: str,
    target: str,
    relation: RelationType = typer.Option(RelationType.RELATES_TO, "--type"),
    workspace: str | None = workspace_option(),
) -> None:
    """Relate two tasks (relates_to, duplicates, follows_up, regression_of, similar_to)."""
    out = STATE.app().tasks.add_relation(
        STATE.ctx(), ws(workspace), ref, RelationCreate(target=target, relation_type=relation)
    )
    emit(out, lambda r: console.print(f"related ({r.relation_type})"))


@task_app.command("link-repo")
@handle_errors
def task_link_repo(
    ref: str,
    repository: str,
    access: str | None = typer.Option(None, help="read_only/read_write"),
    remove: bool = typer.Option(False),
    workspace: str | None = workspace_option(),
) -> None:
    """Associate a repository with a task."""
    app = STATE.app()
    if remove:
        out = app.tasks.unlink_repository(STATE.ctx(), ws(workspace), ref, repository)
    else:
        out = app.tasks.link_repository(STATE.ctx(), ws(workspace), ref, repository, access)
    emit(out, lambda t: console.print(f"{t.key} touches {len(t.repository_ids)} repositories"))


@task_app.command("archive")
@handle_errors
def task_archive(
    ref: str, undo: bool = typer.Option(False, "--undo"), workspace: str | None = workspace_option()
) -> None:
    """Archive (or unarchive) a task. Records are never deleted."""
    out = STATE.app().tasks.archive(STATE.ctx(), ws(workspace), ref, archived=not undo)
    emit(out, lambda t: console.print(f"{t.key} {'unarchived' if undo else 'archived'}"))


@task_app.command("history")
@handle_errors
def task_history(
    ref: str, event_type: list[str] = typer.Option(None, "--type"), workspace: str | None = workspace_option()
) -> None:
    """Show the immutable event history of a task and its attempts."""
    events = STATE.app().history.task_history(STATE.ctx(), ws(workspace), ref, event_types=event_type or None)
    emit(
        events,
        lambda rows: table(
            [(e.seq, when(e.occurred_at), e.event_type, e.actor, _summ(e.data)) for e in rows],
            ["seq", "when", "event", "actor", "data"],
        ),
    )


def _summ(data: dict[str, Any]) -> str:
    if "from" in data and "to" in data:
        return f"{data['from']} -> {data['to']}" + (f" ({data['reason']})" if data.get("reason") else "")
    if "changes" in data:
        return ", ".join(data["changes"])
    return ", ".join(f"{k}={v}" for k, v in list(data.items())[:3])[:80]


@task_app.command("graph")
@handle_errors
def task_graph(
    root: str | None = typer.Option(None),
    project: str | None = typer.Option(None, "--project", "-p"),
    open_only: bool = typer.Option(False, "--open"),
    workspace: str | None = workspace_option(),
) -> None:
    """Print the dependency/hierarchy graph."""
    graph = STATE.app().tasks.graph(
        STATE.ctx(), ws(workspace), root=root, project=project, include_closed=not open_only
    )
    keys = {n.id: n.key for n in graph.nodes}

    def human(g: Any) -> None:
        table(
            [(n.key, n.status, "BLOCKED" if n.blocked else "", n.title[:60]) for n in g.nodes],
            ["key", "status", "", "title"],
        )
        table(
            [
                (keys[e.source], e.kind if e.kind != "depends_on" else "blocks", keys[e.target])
                for e in g.edges
            ],
            ["from", "edge", "to"],
        )

    emit(graph, human)
