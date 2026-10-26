"""Golden vectors for the CLI's rich rendering (GV-13).

Drives the real ``workgraph.cli.common`` helpers (``table``, ``kv``, ``fail``,
``handle_errors``) against consoles whose width, colour and environment are forced
here, so the bytes do not depend on the terminal that runs the script.

Usage: uv run python tools/golden/gen_rich.py

Writes ``golden/rich/{tables,empty_tables,kv,markup,errors}.json`` with sorted keys,
``\\n`` line endings and UTF-8.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
from collections.abc import Callable, Iterable
from pathlib import Path
from typing import Any

# This file is named rich.py, so running it puts tools/golden/ first on sys.path and the
# real `rich` package would be shadowed by this script. Drop that entry before importing.
if sys.path and Path(sys.path[0]).resolve() == Path(__file__).resolve().parent:
    del sys.path[0]

import typer
from rich.errors import MarkupError

from workgraph.cli import common
from workgraph.core.errors import (
    ConflictError,
    DependencyCycleError,
    NotFoundError,
    PermissionDenied,
    ValidationFailed,
)

ROOT = Path(__file__).resolve().parent.parent.parent
OUT_DIR = ROOT / "golden" / "rich"

WIDTHS = (40, 80, 120, 200)

# Cell pool: ASCII, long unbreakable tokens, CJK and emoji (wide cells), combining marks,
# tabs, empty and missing values, numbers, and text that looks like rich markup.
CELLS: tuple[Any, ...] = (
    "WG-12",
    "Implement the lifecycle transition table for attempts and tasks across all states",
    "数据库迁移の確認と適用",
    "🚀 ship it 🚀",
    "naïve café",
    "école",
    "tab\there",
    None,
    "",
    0,
    1234567,
    "x" * 60,
    "[bold]literal[/bold]",
    "[red]not a tag",
    "trailing bracket]",
    "word " * 12,
)

HEADERS: tuple[str, ...] = (
    "id",
    "title",
    "status",
    "owner",
    "updated",
    "priority",
    "размер",
    "标签",
)

TITLES: tuple[str | None, ...] = (None, "Tasks")


def render(width: int, emit: Callable[..., object], *args: Any) -> dict[str, Any]:
    """Run ``emit`` with stdout/stderr consoles forced to ``width``, plain text, no colour.

    Returns the captured ``stdout`` and ``stderr`` text, plus ``exception`` when rendering raised.
    """
    out = io.StringIO()
    err = io.StringIO()
    settings: dict[str, Any] = {
        "width": width,
        "height": 60,
        "color_system": None,
        "force_terminal": False,
        "force_jupyter": False,
        "force_interactive": False,
        "no_color": True,
        "legacy_windows": False,
        "highlight": False,
        "soft_wrap": False,
        "markup": True,
        "emoji": True,
        "tab_size": 8,
        "_environ": {},
    }
    saved = (common.console, common.err_console, common.STATE.json)
    common.console = common.CliConsole(file=out, **settings)
    common.err_console = common.CliConsole(file=err, stderr=True, **settings)
    common.STATE.json = False
    result: dict[str, Any] = {}
    try:
        emit(*args)
    except MarkupError as exc:
        # Malformed markup in a cell or value is a real outcome of the Python renderer;
        # record it so the Rust side must reproduce the same failure.
        result["exception"] = f"MarkupError: {exc}"
    finally:
        common.console, common.err_console, common.STATE.json = saved
    result["stdout"] = out.getvalue()
    result["stderr"] = err.getvalue()
    return result


def table_rows(ncols: int, nrows: int, offset: int) -> list[list[Any]]:
    rows: list[list[Any]] = []
    for r in range(nrows):
        row = []
        for c in range(ncols):
            row.append(CELLS[(r * 7 + c * 3 + offset) % len(CELLS)])
        rows.append(row)
    return rows


def table_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for ncols in range(1, 9):
        headers = list(HEADERS[:ncols])
        for width in WIDTHS:
            for title in TITLES:
                rows = table_rows(ncols, 4, offset=ncols + width)
                cases.append(
                    {
                        "id": f"table-{ncols}col-w{width}-{'titled' if title else 'untitled'}",
                        "width": width,
                        "title": title,
                        "headers": headers,
                        "rows": rows,
                        "output": render(width, common.table, rows, headers, title),
                    }
                )
    return cases


def empty_table_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for ncols in range(1, 9):
        headers = list(HEADERS[:ncols])
        for width in WIDTHS:
            for title in TITLES:
                cases.append(
                    {
                        "id": f"empty-{ncols}col-w{width}-{'titled' if title else 'untitled'}",
                        "width": width,
                        "title": title,
                        "headers": headers,
                        "rows": [],
                        "output": render(width, common.table, [], headers, title),
                    }
                )
    return cases


def kv_pairs() -> list[tuple[str, Any]]:
    return [
        ("id", "WG-12"),
        ("title", "Implement the lifecycle transition table for attempts and tasks across all states"),
        ("name", "数据库迁移の確認と適用"),
        ("status", "🚀 ship it 🚀"),
        ("labels", "[bold]literal[/bold]"),
        ("bracket", "[red]not a tag"),
        ("missing", None),
        ("empty_list", []),
        ("empty_dict", {}),
        ("zero", 0),
        ("token", "x" * 60),
        ("long", "word " * 20),
    ]


def kv_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for width in WIDTHS:
        for title in TITLES:
            pairs = kv_pairs()
            cases.append(
                {
                    "id": f"kv-w{width}-{'titled' if title else 'untitled'}",
                    "width": width,
                    "title": title,
                    "pairs": [[k, v if isinstance(v, str | int | type(None)) else str(v)] for k, v in pairs],
                    "output": render(width, common.kv, pairs, title),
                }
            )
    return cases


MARKUP_TEXTS: tuple[str, ...] = (
    "[",
    "]",
    "[/",
    "[bold",
    "a [red]b",
    "\\[escaped]",
    "[not a tag]",
    "]]",
    "[[double]]",
    "x [/bold] y",
    "[b]short[/b]",
)


def markup_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for width in WIDTHS:
        for text in MARKUP_TEXTS:
            rows = [[text, text, text]]
            headers = ["markup", "again", "third"]
            cases.append(
                {
                    "id": f"markup-table-w{width}-{len(cases)}",
                    "width": width,
                    "text": text,
                    "headers": headers,
                    "rows": rows,
                    "output": render(width, common.table, rows, headers, "Markup"),
                }
            )
            pairs = [("key", text)]
            cases.append(
                {
                    "id": f"markup-kv-w{width}-{len(cases)}",
                    "width": width,
                    "text": text,
                    "pairs": [["key", text]],
                    "output": render(width, common.kv, pairs),
                }
            )
            cases.append(
                {
                    "id": f"markup-fail-w{width}-{len(cases)}",
                    "width": width,
                    "text": text,
                    "output": render(width, _fail_quietly, text),
                }
            )
    return cases


def _fail_quietly(message: str) -> None:
    with contextlib.suppress(typer.Exit):
        common.fail(message, 2)


def _raise(exc: Exception) -> Callable[[], None]:
    def fn() -> None:
        raise exc

    return fn


def error_cases() -> list[dict[str, Any]]:
    samples: list[tuple[str, Exception]] = [
        ("not-found", NotFoundError("task WG-404 not found", details={"task": "WG-404"})),
        (
            "validation",
            ValidationFailed(
                "title must not be empty",
                details={"field": "title", "value": "", "errors": [{"loc": ["title"]}]},
            ),
        ),
        (
            "conflict",
            ConflictError("version changed", details={"expected": 3, "actual": 4, "task": "WG-1"}),
        ),
        ("permission", PermissionDenied("config:manage required", details={"needed": "config:manage"})),
        ("cycle", DependencyCycleError("dependency cycle: WG-1 -> WG-2 -> WG-1")),
        (
            "unicode",
            NotFoundError("任务 WG-9 不存在", details={"任务": "数据库", "emoji": "🚀"}),
        ),
        ("markup", NotFoundError("[bold]not found[/bold] [red]", details={"[key]": "[value]"})),
    ]
    cases: list[dict[str, Any]] = []
    for width in WIDTHS:
        for name, exc in samples:
            cases.append(
                {
                    "id": f"error-{name}-w{width}",
                    "width": width,
                    "code": exc.code,  # type: ignore[attr-defined]
                    "message": exc.message,  # type: ignore[attr-defined]
                    "details": exc.details,  # type: ignore[attr-defined]
                    "output": render(width, _handled_capture, exc),
                }
            )
    cases.append(
        {
            "id": "error-json-mode-w80",
            "width": 80,
            "output": _json_mode_error(),
        }
    )
    return cases


def _handled_capture(exc: Exception) -> None:
    with contextlib.suppress(typer.Exit):
        common.handle_errors(_raise(exc))()


def _json_mode_error() -> dict[str, Any]:
    # JSON mode writes the error envelope to sys.stdout directly, so capture it by redirection.
    buf = io.StringIO()
    code = None
    common.STATE.json = True
    try:
        with contextlib.redirect_stdout(buf):
            try:
                common.handle_errors(
                    _raise(NotFoundError("task WG-404 not found", details={"task": "WG-404"}))
                )()
            except typer.Exit as exc:
                code = exc.exit_code
    finally:
        common.STATE.json = False
    return {"stdout": buf.getvalue(), "stderr": "", "exit_code": code}


EMOJI_TEXTS: tuple[str, ...] = (
    ":rocket: ship it",
    "[bold]:rocket:[/bold] done",
    ":thumbs_up-emoji: :thumbs_up-text: :thumbs_up:",
    ":ROCKET: and :Rocket:",
    ":nope: stays as typed",
    "::rocket: and ::",
    ":rocket-emoji: :x-emoji:",
    ":rocket",
    "数据 :rocket: 完成 :flag_japan:",
    "\\:rocket: and [:rocket:]",
    "[red]:warning:[/red] :smile:",
    ":keycap_#: and :a_button_(blood_type):",
    "a :b c:d: e",
    "::",
)


def emoji_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for width in WIDTHS:
        for text in EMOJI_TEXTS:
            rows = [[text, text, text]]
            headers = ["emoji", "again", "third"]
            cases.append(
                {
                    "id": f"emoji-table-w{width}-{len(cases)}",
                    "width": width,
                    "title": text,
                    "headers": headers,
                    "rows": rows,
                    "output": render(width, common.table, rows, headers, text),
                }
            )
            pairs = [("key", text)]
            cases.append(
                {
                    "id": f"emoji-kv-w{width}-{len(cases)}",
                    "width": width,
                    "text": text,
                    "pairs": [["key", text]],
                    "output": render(width, common.kv, pairs),
                }
            )
    return cases


GROUPS: dict[str, Callable[[], Iterable[dict[str, Any]]]] = {
    "tables": table_cases,
    "empty_tables": empty_table_cases,
    "kv": kv_cases,
    "markup": markup_cases,
    "errors": error_cases,
    "emoji": emoji_cases,
}


def dump(path: Path, cases: list[dict[str, Any]]) -> None:
    doc = {"forced": {"color_system": None, "no_color": True, "_environ": {}, "highlight": False}}
    doc["cases"] = cases
    text = json.dumps(doc, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    path.write_bytes(text.encode("utf-8"))


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, build in GROUPS.items():
        cases = list(build())
        dump(OUT_DIR / f"{name}.json", cases)
        print(f"{name}: {len(cases)} cases")


if __name__ == "__main__":
    main()
