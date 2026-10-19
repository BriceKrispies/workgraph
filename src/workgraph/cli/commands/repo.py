"""Repository registry commands."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    handle_errors,
    kv,
    short,
    table,
    when,
    workspace_option,
    ws,
)
from workgraph.core.models.enums import AccessMode
from workgraph.core.models.schemas import RepositoryRegister, RepositoryUpdate

repo_app = typer.Typer(
    help="Register and inspect Git repositories (files on disk are never deleted).", no_args_is_help=True
)


def _render_repo(r: Any) -> None:
    kv(
        [
            ("id", r.id),
            ("name", r.name),
            ("access", r.access_mode),
            ("health", r.health_status),
            ("default branch", r.default_branch),
            ("tracked", ", ".join(r.tracked_branches)),
            ("fingerprint", short(r.identity_fingerprint, 16)),
            ("shallow", r.is_shallow or None),
            ("last scanned", when(r.last_scanned_at)),
            ("archived", when(r.archived_at) or None),
        ],
        title=r.name,
    )
    table(
        [
            (c.path, c.kind, "yes" if c.is_primary else "", "yes" if c.available else "NO")
            for c in r.checkouts
        ],
        ["checkout", "kind", "primary", "available"],
    )
    if r.remotes:
        table([(m.name, m.url) for m in r.remotes], ["remote", "url"])


@repo_app.command("register")
@handle_errors
def repo_register(
    path: str,
    name: str | None = typer.Option(None, help="Repository name (default: directory name)"),
    description: str = typer.Option(""),
    read_only: bool = typer.Option(
        False, "--read-only", help="Agents may read but never write this repository"
    ),
    branch: list[str] = typer.Option(None, "--branch", help="Tracked branch (repeatable)"),
    project: list[str] = typer.Option(None, "--project", "-p", help="Add to project (repeatable)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Register a local Git repository (or add another checkout of a known one)."""
    out = STATE.app().repositories.register(
        STATE.ctx(),
        ws(workspace),
        RepositoryRegister(
            path=path,
            name=name,
            description=description,
            access_mode=AccessMode.READ_ONLY if read_only else AccessMode.READ_WRITE,
            tracked_branches=branch or [],
            projects=project or [],
        ),
    )
    emit(out, _render_repo)


@repo_app.command("list")
@handle_errors
def repo_list(
    workspace: str | None = workspace_option(),
    all_: bool = typer.Option(False, "--all"),
    project: str | None = typer.Option(None, "--project", "-p"),
) -> None:
    """List registered repositories."""
    items = STATE.app().repositories.list(STATE.ctx(), ws(workspace), include_archived=all_, project=project)
    emit(
        items,
        lambda rows: table(
            [
                (
                    r.name,
                    r.access_mode,
                    r.health_status,
                    len(r.checkouts),
                    r.checkouts[0].path if r.checkouts else "",
                    when(r.last_scanned_at),
                )
                for r in rows
            ],
            ["name", "access", "health", "checkouts", "primary path", "scanned"],
        ),
    )


@repo_app.command("show")
@handle_errors
def repo_show(ref: str, workspace: str | None = workspace_option()) -> None:
    """Inspect a repository registration."""
    emit(STATE.app().repositories.get(STATE.ctx(), ws(workspace), ref), _render_repo)


@repo_app.command("refresh")
@handle_errors
def repo_refresh(ref: str, workspace: str | None = workspace_option()) -> None:
    """Re-check checkouts, remotes and health."""
    emit(STATE.app().repositories.refresh(STATE.ctx(), ws(workspace), ref), _render_repo)


@repo_app.command("status")
@handle_errors
def repo_status(ref: str, workspace: str | None = workspace_option()) -> None:
    """Show working-tree state (branch, staged, unstaged, untracked) of the primary checkout."""
    status = STATE.app().repositories.status(STATE.ctx(), ws(workspace), ref)
    emit(
        status,
        lambda s: kv(
            [
                ("path", s.checkout_path),
                ("available", s.available),
                ("branch", s.branch),
                ("head", short(s.head, 12)),
                ("ahead", s.ahead),
                ("behind", s.behind),
                ("staged", s.staged),
                ("unstaged", s.unstaged),
                ("untracked", s.untracked),
                ("error", s.error),
            ],
            title=s.name,
        ),
    )


@repo_app.command("update")
@handle_errors
def repo_update(
    ref: str,
    description: str | None = typer.Option(None),
    access: AccessMode | None = typer.Option(None),
    default_branch: str | None = typer.Option(None),
    track: list[str] = typer.Option(None, "--track", help="Track a branch for git sync (repeatable)"),
    untrack: list[str] = typer.Option(None, "--untrack", help="Stop tracking a branch (repeatable)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Update registration metadata. --track/--untrack edit the tracked branches git sync scans."""
    app = STATE.app()
    tracked: list[str] | None = None
    if track or untrack:
        tracked = list(app.repositories.get(STATE.ctx(), ws(workspace), ref).tracked_branches)
        for branch in track or []:
            if branch not in tracked:
                tracked.append(branch)
        tracked = [branch for branch in tracked if branch not in set(untrack or [])]
    emit(
        app.repositories.update(
            STATE.ctx(),
            ws(workspace),
            ref,
            RepositoryUpdate(
                description=description,
                access_mode=access,
                default_branch=default_branch,
                tracked_branches=tracked,
            ),
        ),
        _render_repo,
    )


@repo_app.command("remove")
@handle_errors
def repo_remove(ref: str, workspace: str | None = workspace_option()) -> None:
    """Remove a registration. The Git repository on disk is not touched."""
    out = STATE.app().repositories.remove(STATE.ctx(), ws(workspace), ref)
    emit(out, lambda r: console.print(f"removed registration of {r.name}; files on disk untouched"))


@repo_app.command("remove-checkout")
@handle_errors
def repo_remove_checkout(ref: str, path: str, workspace: str | None = workspace_option()) -> None:
    """Forget one checkout path of a repository (files untouched)."""
    emit(STATE.app().repositories.remove_checkout(STATE.ctx(), ws(workspace), ref, path), _render_repo)


@repo_app.command("depend")
@handle_errors
def repo_depend(
    ref: str,
    depends_on: str,
    description: str = typer.Option(""),
    remove: bool = typer.Option(False, help="Remove the declared dependency"),
    workspace: str | None = workspace_option(),
) -> None:
    """Declare that one repository depends on another."""
    app = STATE.app()
    if remove:
        out = app.repositories.remove_dependency(STATE.ctx(), ws(workspace), ref, depends_on)
    else:
        out = app.repositories.add_dependency(STATE.ctx(), ws(workspace), ref, depends_on, description)
    emit(out, _render_repo)
