"""init, doctor, config, workspace and project commands."""

from __future__ import annotations

import json
import platform
import shutil
import sqlite3
import sys
from typing import Any, cast

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    fail,
    handle_errors,
    kv,
    table,
    when,
    workspace_option,
    ws,
)
from workgraph.core.models.schemas import ProjectCreate, ProjectUpdate, WorkspaceCreate

config_app = typer.Typer(help="Inspect, validate and migrate configuration.", no_args_is_help=True)
workspace_app = typer.Typer(help="Manage workspaces.", no_args_is_help=True)
project_app = typer.Typer(help="Manage projects spanning repositories.", no_args_is_help=True)


@handle_errors
def init_command(
    workspace: str = typer.Option("default", "--workspace", "-w", help="Slug of the first workspace"),
    name: str | None = typer.Option(None, help="Display name of the first workspace"),
    prefix: str = typer.Option("WG", help="Task key prefix, e.g. WG -> WG-1"),
    no_workspace: bool = typer.Option(False, help="Only create storage; do not create a workspace"),
) -> None:
    """Create local storage, configuration, the database schema and a first workspace."""
    from workgraph.persistence import migrate

    app = STATE.app()
    if not app.paths.config_file.exists():
        app.config_store.save_global(app.config)
    migrate.upgrade(app.db)
    app.api_token()
    created = None
    if not no_workspace:
        existing = {w.slug for w in app.workspaces.list(STATE.ctx(), include_archived=True)}
        if workspace not in existing:
            created = app.workspaces.create(
                STATE.ctx(),
                WorkspaceCreate(slug=workspace, name=name or workspace.title(), key_prefix=prefix),
                make_default=True,
            )
    result = {
        "config_dir": str(app.paths.config_dir),
        "data_dir": str(app.paths.data_dir),
        "database": str(app.paths.database_file),
        "schema_revision": migrate.current_revision(app.db),
        "workspace_created": created.slug if created else None,
        "default_workspace": app.config.default_workspace,
    }

    def human(r: dict[str, Any]) -> None:
        console.print("[green]Workgraph initialised[/green]")
        kv(r.items())

    emit(result, human)


@handle_errors
def doctor_command(
    verify_events: bool = typer.Option(False, help="Replay the event log and compare with current state"),
) -> None:
    """Check system health: storage, schema, Git, providers, workers and integrations."""
    from workgraph.persistence import migrate

    app = STATE.app()
    checks: list[dict[str, Any]] = []

    def check(name: str, ok: bool, detail: str = "", level: str = "error") -> None:
        checks.append({"check": name, "ok": ok, "level": "ok" if ok else level, "detail": detail})

    check("python", sys.version_info >= (3, 12), platform.python_version())
    git = shutil.which("git")
    if git:
        import subprocess

        version = subprocess.run(
            [git, "--version"], capture_output=True, text=True, check=False
        ).stdout.strip()
        check("git", True, version)
    else:
        check("git", False, "git not found on PATH")
    current = migrate.current_revision(app.db)
    head = migrate.head_revision()
    check("schema", current == head, f"revision {current} (head {head})")
    conn = sqlite3.connect(app.paths.database_file)
    try:
        integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
        mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
    finally:
        conn.close()
    check("database integrity", integrity == "ok", integrity)
    check("journal mode", mode == "wal", mode, level="warning")
    problems = app.config_store.validate_files()
    check("configuration", not problems, "; ".join(problems) or str(app.paths.config_file))
    check("api token", app.paths.api_token_file.exists(), str(app.paths.api_token_file), level="warning")
    try:
        from workgraph.analytics.code.structure import available_languages

        langs = available_languages()
        check("tree-sitter grammars", bool(langs), ", ".join(langs), level="warning")
    except Exception as exc:  # grammar import failures must not crash doctor
        check("tree-sitter grammars", False, str(exc), level="warning")
    for name, status in app.llm.health().items():
        check(f"llm provider {name}", status["available"], status["detail"], level="warning")
    for name, status in app.agents.health().items():
        check(f"agent adapter {name}", status["available"], status["detail"], level="warning")
    check(
        "agent execution",
        True,
        "enabled"
        if app.config.execution.enabled
        else "disabled (opt in with `workgraph config set execution.enabled true`)",
    )
    workers = app.queue.workers()
    live = [w for w in workers if w.status == "running"]
    check("workers", bool(live), f"{len(live)} running", level="warning")
    stats = app.queue.stats()
    check("dead-letter jobs", stats.get("dead", 0) == 0, f"{stats.get('dead', 0)} dead jobs", level="warning")
    spooled = len(list(app.paths.spool_dir.glob("*.json")))
    check("hook spool", spooled == 0, f"{spooled} spooled hook events awaiting replay", level="warning")
    from workgraph.api.app import static_dir

    check("web frontend build", (static_dir() / "index.html").exists(), str(static_dir()), level="warning")
    if verify_events:
        for workspace in app.workspaces.list(STATE.ctx()):
            report = app.history.verify_task_projection(STATE.ctx(), workspace.slug)
            check(
                f"event projection {workspace.slug}",
                report["consistent"],
                f"{report['tasks']} tasks / {report['events']} events"
                + ("" if report["consistent"] else f"; mismatches: {list(report['mismatches'])[:5]}"),
            )
    healthy = all(c["ok"] or c["level"] != "error" for c in checks)

    def human(_: Any) -> None:
        for c in checks:
            mark = (
                "[green]ok[/green]"
                if c["ok"]
                else ("[yellow]warn[/yellow]" if c["level"] == "warning" else "[red]FAIL[/red]")
            )
            console.print(f"{mark:>22}  {c['check']}: [dim]{c['detail']}[/dim]")
        console.print("[green]healthy[/green]" if healthy else "[red]problems found[/red]")

    emit({"healthy": healthy, "checks": checks}, human)
    if not healthy:
        raise typer.Exit(1)


# ---------------------------------------------------------------------------- config


@config_app.command("show")
@handle_errors
def config_show(workspace: str | None = workspace_option()) -> None:
    """Show the effective configuration (global merged with workspace overrides)."""
    app = STATE.app()
    slug = ws(workspace) or app.config.default_workspace
    config = app.effective_config(slug)
    emit(
        {
            "workspace": slug,
            "config": config.model_dump(mode="json"),
            "files": {
                "global": str(app.paths.config_file),
                "workspace": str(app.config_store.workspace_file(slug)) if slug else None,
            },
        },
        lambda r: console.print_json(json.dumps(r)),
    )


@config_app.command("path")
@handle_errors
def config_path() -> None:
    """Print configuration and data locations."""
    paths = STATE.app().paths
    emit(
        {
            "config_file": str(paths.config_file),
            "config_dir": str(paths.config_dir),
            "data_dir": str(paths.data_dir),
            "log_dir": str(paths.log_dir),
            "database": str(paths.database_file),
        },
        lambda r: kv(r.items()),
    )


@config_app.command("validate")
@handle_errors
def config_validate() -> None:
    """Validate all configuration files against their schema."""
    problems = STATE.app().config_store.validate_files()
    emit(
        {"valid": not problems, "problems": problems},
        lambda r: console.print(
            "[green]configuration is valid[/green]" if r["valid"] else "\n".join(problems)
        ),
    )
    if problems:
        raise typer.Exit(7)


@config_app.command("migrate")
@handle_errors
def config_migrate() -> None:
    """Rewrite configuration files in the current schema version."""
    rewritten = STATE.app().config_store.migrate_files()
    emit(
        {"rewritten": [str(p) for p in rewritten]},
        lambda r: console.print(f"migrated {len(rewritten)} file(s)"),
    )


@config_app.command("set")
@handle_errors
def config_set(
    key: str = typer.Argument(..., help="Dotted key, e.g. execution.enabled or llm.classification.model"),
    value: str = typer.Argument(..., help="Value (parsed as JSON when possible)"),
    workspace: str | None = typer.Option(
        None, "--workspace", "-w", help="Write a workspace override instead"
    ),
) -> None:
    """Set a configuration value (validated before it is written)."""
    from workgraph.config.settings import GlobalConfig, WorkspaceOverrides

    app = STATE.app()
    try:
        parsed: Any = json.loads(value)
    except json.JSONDecodeError:
        parsed = value
    parts = key.split(".")
    if workspace:
        overrides = app.config_store.load_workspace_overrides(workspace).model_dump(
            mode="json", exclude_none=True
        )
        target = overrides
    else:
        target = app.config.model_dump(mode="json")
    cursor = target
    for part in parts[:-1]:
        cursor = cursor.setdefault(part, {})
        if not isinstance(cursor, dict):
            fail(f"{key} does not address a nested setting", 2)
    cursor[parts[-1]] = parsed
    from pydantic import ValidationError

    try:
        if workspace:
            ov = WorkspaceOverrides.model_validate(target)
            app.config_store.save_workspace_overrides(workspace, ov)
            app.config_store.effective(workspace)
        else:
            app.config_store.save_global(GlobalConfig.model_validate(target))
    except ValidationError as exc:
        fail(f"invalid value for {key}: {exc.errors()[0]['msg']}", 7)
    emit(
        {"key": key, "value": parsed, "workspace": workspace},
        lambda r: console.print(f"set {key} = {parsed!r}"),
    )


# ---------------------------------------------------------------------------- workspace


@workspace_app.command("create")
@handle_errors
def workspace_create(
    slug: str,
    name: str | None = typer.Option(None),
    prefix: str = typer.Option("WG", help="Task key prefix"),
    description: str = typer.Option(""),
    default: bool = typer.Option(False, "--default", help="Make this the default workspace"),
) -> None:
    """Create a workspace."""
    out = STATE.app().workspaces.create(
        STATE.ctx(),
        WorkspaceCreate(slug=slug, name=name or slug.title(), key_prefix=prefix, description=description),
        make_default=default,
    )
    emit(out, lambda w: console.print(f"created workspace [bold]{w.slug}[/bold] ({w.id})"))


@workspace_app.command("list")
@handle_errors
def workspace_list(all_: bool = typer.Option(False, "--all", help="Include archived")) -> None:
    """List workspaces."""
    app = STATE.app()
    items = app.workspaces.list(STATE.ctx(), include_archived=all_)
    default = app.config.default_workspace
    emit(
        items,
        lambda rows: table(
            [
                ("*" if w.slug == default else "", w.slug, w.name, w.key_prefix, when(w.created_at))
                for w in rows
            ],
            ["", "slug", "name", "prefix", "created"],
        ),
    )


@workspace_app.command("show")
@handle_errors
def workspace_show(ref: str | None = typer.Argument(None)) -> None:
    """Show a workspace summary."""
    app = STATE.app()
    summary = app.workspaces.summary(STATE.ctx(), ref or ws(None) or app.default_workspace_id())

    def human(s: Any) -> None:
        kv(
            [
                ("id", s.workspace.id),
                ("slug", s.workspace.slug),
                ("name", s.workspace.name),
                ("projects", s.projects),
                ("repositories", s.repositories),
                ("ready tasks", s.ready_tasks),
                ("running attempts", s.running_attempts),
                ("queued jobs", s.queued_jobs),
                ("dead jobs", s.dead_jobs),
                ("tasks by status", s.tasks_by_status),
            ],
            title="workspace",
        )

    emit(summary, human)


@workspace_app.command("use")
@handle_errors
def workspace_use(ref: str) -> None:
    """Set the default workspace."""
    out = STATE.app().workspaces.set_default(STATE.ctx(), ref)
    emit(out, lambda w: console.print(f"default workspace is now [bold]{w.slug}[/bold]"))


@workspace_app.command("archive")
@handle_errors
def workspace_archive(ref: str) -> None:
    """Archive a workspace (records are kept)."""
    emit(STATE.app().workspaces.archive(STATE.ctx(), ref), lambda w: console.print(f"archived {w.slug}"))


@workspace_app.command("set")
@handle_errors
def workspace_set(
    ref: str,
    allowed_root: list[str] = typer.Option(
        None, "--allowed-root", help="Restrict repository paths (repeatable)"
    ),
) -> None:
    """Update workspace settings such as allowed repository roots."""
    settings: dict[str, Any] = {}
    if allowed_root:
        settings["allowed_roots"] = allowed_root
    if not settings:
        fail("nothing to update", 2)
    emit(STATE.app().workspaces.update_settings(STATE.ctx(), ref, settings), lambda w: kv(w.settings.items()))


# ---------------------------------------------------------------------------- project


@project_app.command("create")
@handle_errors
def project_create(
    slug: str,
    name: str | None = typer.Option(None),
    description: str = typer.Option(""),
    repo: list[str] = typer.Option(None, "--repo", "-r", help="Repository name or id (repeatable)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Create a project, optionally spanning repositories."""
    app = STATE.app()
    out = app.projects.create(
        STATE.ctx(),
        ws(workspace) or app.default_workspace_id(),
        ProjectCreate(
            slug=slug,
            name=name or slug.replace("-", " ").title(),
            description=description,
            repositories=repo or [],
        ),
    )
    emit(
        out,
        lambda p: console.print(
            f"created project [bold]{p.slug}[/bold] with {len(p.repository_ids)} repositories"
        ),
    )


@project_app.command("list")
@handle_errors
def project_list(
    workspace: str | None = workspace_option(), all_: bool = typer.Option(False, "--all")
) -> None:
    """List projects."""
    app = STATE.app()
    items = app.projects.list(STATE.ctx(), ws(workspace) or app.default_workspace_id(), include_archived=all_)
    emit(
        items,
        lambda rows: table(
            [(p.slug, p.name, p.status, len(p.repository_ids)) for p in rows],
            ["slug", "name", "status", "repos"],
        ),
    )


@project_app.command("show")
@handle_errors
def project_show(ref: str, workspace: str | None = workspace_option()) -> None:
    """Show a project summary."""
    app = STATE.app()
    summary = app.projects.summary(STATE.ctx(), ws(workspace) or app.default_workspace_id(), ref)

    def human(s: Any) -> None:
        kv(
            [
                ("id", s.project.id),
                ("name", s.project.name),
                ("status", s.project.status),
                ("repositories", ", ".join(r.name for r in s.repositories)),
                ("open tasks", s.open_tasks),
                ("accepted", s.accepted_tasks),
                ("by status", s.tasks_by_status),
            ],
            title=s.project.slug,
        )

    emit(summary, human)


@project_app.command("update")
@handle_errors
def project_update(
    ref: str,
    name: str | None = typer.Option(None),
    description: str | None = typer.Option(None),
    status: str | None = typer.Option(None, help="active or archived"),
    workspace: str | None = workspace_option(),
) -> None:
    """Update project fields."""
    app = STATE.app()
    out = app.projects.update(
        STATE.ctx(),
        ws(workspace) or app.default_workspace_id(),
        ref,
        ProjectUpdate(name=name, description=description, status=cast(Any, status)),
    )
    emit(out, lambda p: console.print(f"updated {p.slug}"))


@project_app.command("add-repo")
@handle_errors
def project_add_repo(
    ref: str, repository: str, role: str = typer.Option("member"), workspace: str | None = workspace_option()
) -> None:
    """Associate a repository with a project."""
    app = STATE.app()
    out = app.projects.add_repository(
        STATE.ctx(), ws(workspace) or app.default_workspace_id(), ref, repository, role
    )
    emit(out, lambda p: console.print(f"{p.slug} now spans {len(p.repository_ids)} repositories"))


@project_app.command("remove-repo")
@handle_errors
def project_remove_repo(ref: str, repository: str, workspace: str | None = workspace_option()) -> None:
    """Remove a repository from a project."""
    app = STATE.app()
    out = app.projects.remove_repository(
        STATE.ctx(), ws(workspace) or app.default_workspace_id(), ref, repository
    )
    emit(out, lambda p: console.print(f"{p.slug} now spans {len(p.repository_ids)} repositories"))
