"""Shared CLI state, output formatting and error handling.

Every command supports ``--json`` (anywhere on the command line) for stable,
machine-readable output, and human-readable tables otherwise. Exit codes:

====  =====================================
0     success
1     unexpected error
2     validation failed / bad usage
3     conflict (state, version, busy resource)
4     not found
5     permission denied
6     provider unavailable
7     configuration error
====  =====================================
"""

from __future__ import annotations

import json
import os
import sys
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field
from datetime import datetime
from functools import wraps
from pathlib import Path
from typing import TYPE_CHECKING, Any, NoReturn, ParamSpec, TypeVar

import typer
from pydantic import BaseModel
from rich.console import Console
from rich.table import Table

from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConfigError,
    ConflictError,
    NotFoundError,
    PermissionDenied,
    ProviderUnavailableError,
    ValidationFailed,
    WorkgraphError,
)

if TYPE_CHECKING:
    from workgraph.app import Workgraph

P = ParamSpec("P")
R = TypeVar("R")

EXIT_CODES: list[tuple[type[WorkgraphError], int]] = [
    (ValidationFailed, 2),
    (ConflictError, 3),
    (NotFoundError, 4),
    (PermissionDenied, 5),
    (ProviderUnavailableError, 6),
    (ConfigError, 7),
]

console = Console(highlight=False, soft_wrap=False)
err_console = Console(stderr=True, highlight=False)


@dataclass
class CLIState:
    json: bool = False
    home: Path | None = None
    workspace: str | None = None
    _app: Any = field(default=None, repr=False)

    def app(self) -> Workgraph:
        if self._app is None:
            from workgraph.app import Workgraph
            from workgraph.config.paths import AppPaths

            self._app = Workgraph(AppPaths.resolve(self.home))
        return self._app

    def ctx(self) -> RequestContext:
        return RequestContext.human()

    def close(self) -> None:
        if self._app is not None:
            self._app.close()
            self._app = None


STATE = CLIState()


def to_jsonable(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return value.model_dump(mode="json")
    if isinstance(value, dict):
        return {str(k): to_jsonable(v) for k, v in value.items()}
    if isinstance(value, list | tuple | set):
        return [to_jsonable(v) for v in value]
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, Path):
        return str(value)
    return value


def emit(value: Any, human: Callable[[Any], object] | None = None) -> None:
    """Print ``value`` as JSON (``--json``) or via the ``human`` renderer."""
    if STATE.json or human is None:
        sys.stdout.write(json.dumps(to_jsonable(value), indent=2, default=str) + "\n")
        sys.stdout.flush()
    else:
        human(value)


def fail(message: str, code: int = 1, details: dict[str, Any] | None = None) -> NoReturn:
    if STATE.json:
        sys.stdout.write(
            json.dumps({"error": {"code": "cli_error", "message": message, "details": details or {}}}) + "\n"
        )
    else:
        err_console.print(f"[red]error:[/red] {message}")
    raise typer.Exit(code)


def handle_errors[**P, R](fn: Callable[P, R]) -> Callable[P, R]:
    """Translate domain exceptions into consistent output and exit codes."""

    @wraps(fn)
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        try:
            return fn(*args, **kwargs)
        except typer.Exit:
            raise
        except WorkgraphError as exc:
            code = next((c for cls, c in EXIT_CODES if isinstance(exc, cls)), 1)
            if STATE.json:
                sys.stdout.write(json.dumps({"error": exc.to_dict()}, default=str) + "\n")
            else:
                err_console.print(f"[red]error[/red] ({exc.code}): {exc.message}")
                for key, value in exc.details.items():
                    if key != "errors":
                        err_console.print(f"  {key}: {value}")
            raise typer.Exit(code) from exc
        except KeyboardInterrupt:
            raise typer.Exit(130) from None

    return wrapper


def table(rows: Iterable[Sequence[Any]], headers: Sequence[str], title: str | None = None) -> None:
    t = Table(title=title, show_lines=False, header_style="bold")
    for header in headers:
        t.add_column(header, overflow="fold")
    count = 0
    for row in rows:
        t.add_row(*["" if v is None else str(v) for v in row])
        count += 1
    if count == 0:
        console.print(f"[dim]{title + ': ' if title else ''}nothing to show[/dim]")
        return
    console.print(t)


def kv(pairs: Iterable[tuple[str, Any]], title: str | None = None) -> None:
    if title:
        console.print(f"[bold]{title}[/bold]")
    for key, value in pairs:
        if value is None or value in ([], {}):
            continue
        console.print(f"  [cyan]{key}[/cyan]: {value}")


def short(value: str | None, n: int = 8) -> str:
    return value[:n] if value else ""


def when(value: datetime | None) -> str:
    return value.strftime("%Y-%m-%d %H:%M") if value else ""


def workspace_option() -> Any:
    return typer.Option(
        None,
        "--workspace",
        "-w",
        envvar="WORKGRAPH_WORKSPACE",
        help="Workspace slug or id (default: configured)",
    )


def ws(override: str | None) -> str | None:
    return override or STATE.workspace or os.environ.get("WORKGRAPH_WORKSPACE")


def parse_json_arg(value: str | None, what: str) -> Any:
    if value is None:
        return None
    text = value
    if value.startswith("@"):
        text = Path(value[1:]).read_text(encoding="utf-8")
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        fail(f"{what} is not valid JSON: {exc}", 2)
