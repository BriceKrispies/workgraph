"""git commands: ingestion, attribution, diffs, worktrees and optional hooks."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import typer

from workgraph.cli.common import STATE, console, emit, handle_errors, kv, short, table, workspace_option, ws

git_app = typer.Typer(help="Git ingestion, commit attribution, diffs and worktrees.", no_args_is_help=True)


@git_app.command("sync")
@handle_errors
def git_sync(
    repository: str | None = typer.Argument(None, help="Repository (default: all)"),
    full: bool = typer.Option(False, "--full", help="Full reconciliation (rewritten history, reachability)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Import new commits (incremental) or reconcile full history."""
    app = STATE.app()
    mode = "full" if full else "incremental"
    results = (
        [app.git.sync_ref(STATE.ctx(), ws(workspace), repository, mode=mode)]
        if repository
        else app.git.sync_workspace(STATE.ctx(), ws(workspace), mode=mode)
    )
    emit(
        results,
        lambda rs: table(
            [
                (
                    r.repository,
                    r.mode,
                    r.new_commits,
                    r.refs_changed,
                    r.marked_unreachable,
                    r.equivalences,
                    r.links_created,
                    "yes" if r.shallow else "",
                )
                for r in rs
            ],
            ["repository", "mode", "new", "refs", "unreachable", "equivalent", "links", "shallow"],
        ),
    )


@git_app.command("log")
@handle_errors
def git_log(
    repository: str,
    limit: int = typer.Option(30),
    linked: bool = typer.Option(False, "--linked"),
    workspace: str | None = workspace_option(),
) -> None:
    """Ingested commits with their task links."""
    data = STATE.app().git.repository_commits(
        STATE.ctx(), ws(workspace), repository, limit=limit, linked_only=linked
    )
    emit(
        data,
        lambda d: table(
            [
                (
                    c["sha"][:10],
                    c["subject"][:60],
                    c["author"],
                    f"+{c['insertions']}/-{c['deletions']}",
                    ",".join(t["task_key"] for t in c["tasks"]),
                )
                for c in d["items"]
            ],
            ["sha", "subject", "author", "lines", "tasks"],
        ),
    )


@git_app.command("show")
@handle_errors
def git_show(repository: str, sha: str, workspace: str | None = workspace_option()) -> None:
    """Commit metadata, files, symbols, equivalences and task links."""
    detail = STATE.app().git.commit_detail(STATE.ctx(), ws(workspace), repository, sha)

    def human(d: dict[str, Any]) -> None:
        kv(
            [
                ("sha", d["sha"]),
                ("subject", d["subject"]),
                ("author", d["author"]["name"]),
                ("merge", d["is_merge"]),
                ("reachable", d["reachable"]),
                ("tasks", [t["task_key"] for t in d["tasks"]]),
            ],
            title="commit",
        )
        table(
            [
                (
                    f["change_type"],
                    f["path"],
                    f"+{f['insertions']}/-{f['deletions']}",
                    f["file_class"],
                    ", ".join(s["name"] for s in f["symbols"])[:60],
                )
                for f in d["files"]
            ],
            ["", "path", "lines", "class", "symbols"],
        )

    emit(detail, human)


@git_app.command("link")
@handle_errors
def git_link(
    task: str,
    sha: str,
    repository: str | None = typer.Option(None),
    note: str = typer.Option(""),
    workspace: str | None = workspace_option(),
) -> None:
    """Explicitly link a commit to a task."""
    links = STATE.app().git.link_commit(
        STATE.ctx(), ws(workspace), task, sha, repository=repository, note=note
    )
    emit(links, lambda ls: console.print(f"linked {ls[0].sha[:10]} to task ({len(ls)} link(s))"))


@git_app.command("unlink")
@handle_errors
def git_unlink(task: str, sha: str, workspace: str | None = workspace_option()) -> None:
    """Remove an explicit link (inferred provenance remains as evidence)."""
    removed = STATE.app().git.unlink_commit(STATE.ctx(), ws(workspace), task, sha)
    emit({"removed": removed}, lambda r: console.print(f"removed {removed} explicit link(s)"))


@git_app.command("changes")
@handle_errors
def git_changes(
    task: str, min_confidence: float = typer.Option(0.5), workspace: str | None = workspace_option()
) -> None:
    """Commits, cumulative churn and final change surface of a task."""
    data = STATE.app().git.task_changes(STATE.ctx(), ws(workspace), task, min_confidence=min_confidence)

    def human(d: dict[str, Any]) -> None:
        table(
            [
                (
                    c["repository"],
                    c["sha"][:10],
                    c["subject"][:50],
                    f"{c['confidence']:.2f}",
                    ",".join(sorted({lk["source"] for lk in c["links"]})),
                )
                for c in d["commits"]
            ],
            ["repository", "sha", "subject", "conf", "sources"],
        )
        kv(d["cumulative_churn"].items(), title="cumulative churn")
        for s in d["final_change_surface"]:
            kv([(k, v) for k, v in s.items() if k != "detail"], title="final change surface")

    emit(data, human)


@git_app.command("diff")
@handle_errors
def git_diff(
    task: str, repository: str | None = typer.Option(None), workspace: str | None = workspace_option()
) -> None:
    """Print the full diff for a task (read live from Git)."""
    data = STATE.app().git.task_diff(STATE.ctx(), ws(workspace), task, repository=repository)

    def human(d: dict[str, Any]) -> None:
        for part in d["parts"]:
            console.print(f"[bold]# {part['repository']} {part.get('range') or part.get('sha', '')}[/bold]")
            console.print(part["diff"], markup=False, highlight=False)
        for gap in d["missing_objects"]:
            console.print(f"[yellow]missing Git objects:[/yellow] {gap}")

    emit(data, human)


@git_app.command("surface")
@handle_errors
def git_surface(task: str, workspace: str | None = workspace_option()) -> None:
    """(Re)compute the final change surface of an accepted task."""
    app = STATE.app()
    t = app.tasks.get(STATE.ctx(), ws(workspace), task)
    emit(
        app.git.compute_change_surface(STATE.ctx(), t.id),
        lambda rows: table(
            [
                (
                    short(r["repository_id"]),
                    r["base_sha"][:10],
                    r["final_sha"][:10],
                    r["files_changed"],
                    r["insertions"],
                    r["deletions"],
                    r["functions_affected"],
                )
                for r in rows
            ],
            ["repo", "base", "final", "files", "+", "-", "functions"],
        ),
    )


@git_app.command("analyze")
@handle_errors
def git_analyze(
    repository: str, limit: int = typer.Option(200), workspace: str | None = workspace_option()
) -> None:
    """Run structural (Tree-sitter) analysis on not-yet-analysed commits."""
    app = STATE.app()
    data = app.git.repository_commits(STATE.ctx(), ws(workspace), repository, limit=limit)
    analyzed = app.git.analyze_commits([c["id"] for c in data["items"] if not c["is_merge"]])
    emit({"analyzed": analyzed}, lambda r: console.print(f"analyzed {analyzed} commit(s)"))


@git_app.command("worktrees")
@handle_errors
def git_worktrees(
    status: str | None = typer.Option(None), workspace: str | None = workspace_option()
) -> None:
    """Managed worktrees and their lifecycle state."""
    rows = STATE.app().worktrees.list(STATE.ctx(), ws(workspace), status=status)
    emit(
        rows,
        lambda rs: table(
            [(short(r["id"]), r["repository"], r["status"], r["branch"], r["path"]) for r in rs],
            ["id", "repository", "status", "branch", "path"],
        ),
    )


@git_app.command("cleanup-worktrees")
@handle_errors
def git_cleanup(
    apply: bool = typer.Option(False, "--apply", help="Actually remove (default: dry run)"),
) -> None:
    """Remove released, clean Workgraph-managed worktrees (branches are kept)."""
    emit(
        STATE.app().worktrees.cleanup(STATE.ctx(), dry_run=not apply),
        lambda rows: table(
            [
                (
                    r.get("path", r.get("worktree_id")),
                    r.get("action") or ("removed" if r.get("removed") else r.get("reason")),
                )
                for r in rows
            ],
            ["worktree", "result"],
        ),
    )


HOOK_MARKER = "# workgraph-managed hook"


@git_app.command("install-hook")
@handle_errors
def git_install_hook(repository: str, workspace: str | None = workspace_option()) -> None:
    """Optional: add a post-commit hook that queues an incremental sync (never required)."""
    import shutil

    app = STATE.app()
    repo = app.repositories.get(STATE.ctx(), ws(workspace), repository)
    checkout = next((c for c in repo.checkouts if c.available and c.kind != "managed_worktree"), None)
    if checkout is None:
        from workgraph.cli.common import fail

        fail("no available checkout", 4)
    from workgraph.integrations.git.cli import Git

    hooks_dir = Git(checkout.path).git_common_dir() / "hooks"
    hook = hooks_dir / "post-commit"
    exe = shutil.which("workgraph") or "workgraph"
    if hook.exists() and HOOK_MARKER not in hook.read_text(encoding="utf-8", errors="replace"):
        from workgraph.cli.common import fail

        fail(f"{hook} already exists and is not managed by Workgraph; not overwriting", 3)
    script = (
        f'#!/bin/sh\n{HOOK_MARKER}\n"{Path(exe).as_posix()}" job enqueue git_sync '
        f'--payload \'{{"repository_id": "{repo.id}", "mode": "incremental"}}\' >/dev/null 2>&1 || true\n'
    )
    hook.write_text(script, encoding="utf-8", newline="\n")
    hook.chmod(0o755)
    emit({"hook": str(hook)}, lambda r: console.print(f"installed {hook}"))
