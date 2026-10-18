"""claude commands: hook installation, status, binding and spool replay.

``workgraph claude hook`` itself is dispatched by :mod:`workgraph.cli.entry` before
this module is imported, so it stays fast.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path
from typing import Any, cast

import typer

from workgraph.cli.common import STATE, console, emit, fail, handle_errors, kv, table, workspace_option, ws
from workgraph.integrations.claude_code import installer

claude_app = typer.Typer(
    help="Claude Code integration (passive lifecycle hooks, MCP registration).", no_args_is_help=True
)


@claude_app.command("hook")
def hook(debug: bool = typer.Option(False, "--debug")) -> None:
    """Ingest one Claude Code hook payload from stdin (used by Claude Code; always exits 0)."""
    from workgraph.integrations.claude_code.hook_cli import run_hook

    raise typer.Exit(run_hook(["--debug"] if debug else []))


@claude_app.command("install")
@handle_errors
def install(
    scope: str = typer.Option("user", help="user (~/.claude), project (.claude/settings.json) or local"),
    project_dir: Path | None = typer.Option(None, help="Project directory for project/local scope"),
    command: str | None = typer.Option(None, help="Override the workgraph executable path"),
    event: list[str] = typer.Option(None, "--event", help="Subset of hook events (default: all supported)"),
    mcp: bool = typer.Option(
        False, "--mcp", help="Also register the Workgraph MCP server with `claude mcp add`"
    ),
) -> None:
    """Merge Workgraph's passive hooks into Claude Code settings (other settings are preserved)."""
    if scope not in ("user", "project", "local"):
        fail("scope must be user, project or local", 2)
    STATE.app()  # ensure storage exists so hooks can ingest immediately
    result: dict[str, Any] = installer.install(
        scope,
        project_dir=project_dir,
        command=command,
        events=event or None,
    )
    if mcp:
        exe = shutil.which("claude")
        if exe is None:
            result["mcp"] = "claude CLI not found; add manually: " + json.dumps(
                installer.mcp_server_config(command)
            )
        else:
            mcp_scope = "user" if scope == "user" else ("project" if scope == "project" else "local")
            wg = command or installer.workgraph_executable()
            proc = subprocess.run(
                [exe, "mcp", "add", "--scope", mcp_scope, "workgraph", "--", wg, "mcp"],
                capture_output=True,
                text=True,
                check=False,
                timeout=60,
            )
            result["mcp"] = (proc.stdout or proc.stderr).strip()[:500]
    emit(result, lambda r: kv(r.items(), title="claude install"))


@claude_app.command("uninstall")
@handle_errors
def uninstall(scope: str = typer.Option("user"), project_dir: Path | None = typer.Option(None)) -> None:
    """Remove only Workgraph-owned hook entries."""
    emit(
        installer.uninstall(cast(Any, scope), project_dir=project_dir),
        lambda r: kv(r.items(), title="claude uninstall"),
    )


@claude_app.command("status")
@handle_errors
def status(scope: str = typer.Option("user"), project_dir: Path | None = typer.Option(None)) -> None:
    """Hook installation state, ingestion statistics, spool backlog and recent sessions."""
    app = STATE.app()
    data = {
        "installation": installer.status(cast(Any, scope), project_dir=project_dir),
        "ingestion": app.hooks.stats(),
        "sessions": app.hooks.sessions(limit=10),
    }

    def human(d: dict[str, Any]) -> None:
        kv(d["installation"].items(), title="installation")
        kv(d["ingestion"].items(), title="ingestion")
        table(
            [
                (
                    s["external_session_id"][:12],
                    s["binding"],
                    (s["task_id"] or "")[:8],
                    s["tool_calls"],
                    s["tool_failures"],
                    s["started_at"] or "",
                )
                for s in d["sessions"]
            ],
            ["session", "binding", "task", "tools", "failed", "started"],
        )

    emit(data, human)


@claude_app.command("replay")
@handle_errors
def replay(limit: int = typer.Option(1000)) -> None:
    """Ingest spooled hook events that could not be stored immediately."""
    emit(STATE.app().hooks.replay_spool(limit=limit), lambda r: kv(r.items(), title="replay"))


@claude_app.command("backfill-transcripts")
@handle_errors
def backfill_transcripts(
    limit: int | None = typer.Option(None, help="Process at most this many subagent runs"),
) -> None:
    """Fill subagent model, usage and task links from transcripts (idempotent)."""
    emit(
        STATE.app().hooks.backfill_transcripts(STATE.ctx(), limit=limit),
        lambda r: kv(r.items(), title="backfill-transcripts"),
    )


@claude_app.command("bind")
@handle_errors
def bind(
    task: str,
    session: str | None = typer.Option(None, help="Claude Code session id"),
    cwd: Path | None = typer.Option(
        None, help="Bind sessions started under this directory (default: current)"
    ),
    workspace: str | None = workspace_option(),
) -> None:
    """Bind externally-launched Claude Code sessions to a task."""
    out = STATE.app().hooks.bind(
        STATE.ctx(), ws(workspace), task, session_id=session, cwd=str(cwd) if cwd else None
    )
    emit(out, lambda r: console.print(f"bound {r['session_id'] or r['cwd']} to {r['task']}"))


@claude_app.command("unbind")
@handle_errors
def unbind(binding: str | None = typer.Option(None), cwd: Path | None = typer.Option(None)) -> None:
    """Deactivate session bindings."""
    count = STATE.app().hooks.unbind(
        STATE.ctx(), binding_id=binding, cwd=str(cwd or Path.cwd()) if not binding else None
    )
    emit({"deactivated": count}, lambda r: console.print(f"deactivated {count} binding(s)"))


@claude_app.command("sessions")
@handle_errors
def sessions(limit: int = typer.Option(30)) -> None:
    """Recent Claude Code sessions and their Workgraph bindings."""
    rows = STATE.app().hooks.sessions(limit=limit)
    emit(
        rows,
        lambda rs: table(
            [
                (
                    s["external_session_id"],
                    s["binding"],
                    s["task_id"] or "",
                    s["attempt_id"] or "",
                    s["tool_calls"],
                )
                for s in rs
            ],
            ["session", "binding", "task", "attempt", "tools"],
        ),
    )


@claude_app.command("mcp-config")
@handle_errors
def mcp_config(command: str | None = typer.Option(None)) -> None:
    """Print the MCP client configuration block for Workgraph (stdio)."""
    emit(installer.mcp_server_config(command), lambda r: console.print_json(json.dumps(r)))
