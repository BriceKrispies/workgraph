"""Install/uninstall Workgraph's Claude Code hooks without touching unrelated settings.

* Settings are parsed as JSON; an unparsable file is never overwritten.
* A timestamped backup is written before every modification, then the new file is
  written atomically.
* Workgraph-owned handlers are recognised by their exec-form command
  (``<workgraph executable>`` with ``args == ["claude", "hook"]``) or the shell-form
  string ``workgraph claude hook``. Uninstall removes only those handlers.
* Hooks are registered with ``"async": true`` and print nothing, so they are passive:
  they cannot block tools, change permissions or inject context.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Literal

from workgraph.core.errors import ConfigError
from workgraph.core.fsutil import atomic_write_text

Scope = Literal["user", "project", "local"]
DEFAULT_EVENTS = [
    "SessionStart",
    "SessionEnd",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
]
HOOK_ARGS = ["claude", "hook"]


def settings_path(scope: Scope, project_dir: Path | None = None) -> Path:
    if scope == "user":
        base = os.environ.get("CLAUDE_CONFIG_DIR")
        return (Path(base) if base else Path.home() / ".claude") / "settings.json"
    root = (project_dir or Path.cwd()).resolve()
    return root / ".claude" / ("settings.json" if scope == "project" else "settings.local.json")


def workgraph_executable() -> str:
    found = shutil.which("workgraph")
    if found:
        return str(Path(found).resolve())
    candidate = Path(sys.executable).parent / ("workgraph.exe" if os.name == "nt" else "workgraph")
    if candidate.exists():
        return str(candidate)
    raise ConfigError("could not locate the workgraph executable; install it or pass --command")


def is_workgraph_handler(handler: dict[str, Any]) -> bool:
    if not isinstance(handler, dict) or handler.get("type") != "command":
        return False
    command = str(handler.get("command", ""))
    args = handler.get("args")
    name = Path(command.replace("\\", "/").split("/")[-1]).stem.lower() if command else ""
    if isinstance(args, list) and args[:2] == HOOK_ARGS and name == "workgraph":
        return True
    return "workgraph claude hook" in command.replace("\\", "/")


def _load(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        return {}
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ConfigError(f"{path} is not valid JSON ({exc}); refusing to modify it") from exc
    if not isinstance(data, dict):
        raise ConfigError(f"{path} does not contain a JSON object; refusing to modify it")
    return data


def _write(path: Path, data: dict[str, Any]) -> Path | None:
    backup = None
    if path.exists():
        stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        backup = path.with_name(f"{path.name}.workgraph-backup-{stamp}")
        shutil.copy2(path, backup)
    atomic_write_text(path, json.dumps(data, indent=2) + "\n")
    return backup


def install(
    scope: Scope = "user",
    *,
    project_dir: Path | None = None,
    events: list[str] | None = None,
    command: str | None = None,
    timeout: int = 10,
) -> dict[str, Any]:
    path = settings_path(scope, project_dir)
    data = _load(path)
    exe = command or workgraph_executable()
    hooks = data.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise ConfigError(f"'hooks' in {path} is not an object; refusing to modify it")
    added: list[str] = []
    for event in events or DEFAULT_EVENTS:
        groups = hooks.setdefault(event, [])
        if not isinstance(groups, list):
            raise ConfigError(f"hooks.{event} in {path} is not a list; refusing to modify it")
        present = any(
            is_workgraph_handler(h) for g in groups if isinstance(g, dict) for h in (g.get("hooks") or [])
        )
        if present:
            continue
        groups.append(
            {
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": exe, "args": HOOK_ARGS, "timeout": timeout, "async": True}
                ],
            }
        )
        added.append(event)
    backup = _write(path, data) if added else None
    return {"settings": str(path), "added": added, "backup": str(backup) if backup else None, "command": exe}


def uninstall(scope: Scope = "user", *, project_dir: Path | None = None) -> dict[str, Any]:
    path = settings_path(scope, project_dir)
    data = _load(path)
    hooks = data.get("hooks")
    removed: list[str] = []
    if isinstance(hooks, dict):
        for event in list(hooks):
            groups = hooks[event]
            if not isinstance(groups, list):
                continue
            new_groups = []
            for group in groups:
                if not isinstance(group, dict):
                    new_groups.append(group)
                    continue
                handlers = group.get("hooks") or []
                kept = [h for h in handlers if not is_workgraph_handler(h)]
                if len(kept) != len(handlers):
                    removed.append(event)
                if kept:
                    new_groups.append({**group, "hooks": kept})
                elif not handlers:
                    new_groups.append(group)
            if new_groups:
                hooks[event] = new_groups
            else:
                del hooks[event]
        if not hooks:
            del data["hooks"]
    backup = _write(path, data) if removed else None
    return {"settings": str(path), "removed": sorted(set(removed)), "backup": str(backup) if backup else None}


def status(scope: Scope = "user", *, project_dir: Path | None = None) -> dict[str, Any]:
    path = settings_path(scope, project_dir)
    try:
        data = _load(path)
        error = None
    except ConfigError as exc:
        data, error = {}, exc.message
    installed: dict[str, str] = {}
    hooks = data.get("hooks") if isinstance(data.get("hooks"), dict) else {}
    for event, groups in (hooks or {}).items():
        for group in groups if isinstance(groups, list) else []:
            for handler in (group.get("hooks") or []) if isinstance(group, dict) else []:
                if is_workgraph_handler(handler):
                    installed[event] = str(handler.get("command"))
    commands_ok = all(Path(c).exists() or shutil.which(c) for c in set(installed.values()))
    return {
        "scope": scope,
        "settings": str(path),
        "exists": path.exists(),
        "error": error,
        "installed_events": sorted(installed),
        "missing_events": sorted(set(DEFAULT_EVENTS) - set(installed)),
        "command_resolves": bool(installed) and commands_ok,
    }


def mcp_server_config(command: str | None = None) -> dict[str, Any]:
    """The JSON block an MCP client needs to launch Workgraph over stdio."""
    exe = command or workgraph_executable()
    return {"mcpServers": {"workgraph": {"type": "stdio", "command": exe, "args": ["mcp"]}}}
