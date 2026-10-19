"""Console-script entry point.

Kept tiny so ``workgraph claude hook`` (run by Claude Code on every lifecycle event)
starts fast: it dispatches to the hook handler before the full CLI is imported.
"""

from __future__ import annotations

import sys


def make_stdio_non_fatal() -> None:
    """Never raise on text the console encoding cannot represent.

    On a cp1252 Windows console, printing ``→`` or ``—`` raises UnicodeEncodeError
    mid-command. Keep whatever encoding the stream already has and only replace the
    characters it cannot encode. A UTF-8 stream is left alone, so ``--json`` output
    keeps its exact bytes.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        encoding = (getattr(stream, "encoding", None) or "").lower().replace("-", "")
        if reconfigure is not None and encoding != "utf8":
            reconfigure(errors="replace")


def main() -> None:
    make_stdio_non_fatal()
    if sys.argv[1:3] == ["claude", "hook"]:
        from workgraph.integrations.claude_code.hook_cli import run_hook

        raise SystemExit(run_hook(sys.argv[3:]))
    from workgraph.cli.main import main as full_main

    full_main()


if __name__ == "__main__":
    main()
