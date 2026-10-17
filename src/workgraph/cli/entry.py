"""Console-script entry point.

Kept tiny so ``workgraph claude hook`` (run by Claude Code on every lifecycle event)
starts fast: it dispatches to the hook handler before the full CLI is imported.
"""

from __future__ import annotations

import sys


def main() -> None:
    if sys.argv[1:3] == ["claude", "hook"]:
        from workgraph.integrations.claude_code.hook_cli import run_hook

        raise SystemExit(run_hook(sys.argv[3:]))
    from workgraph.cli.main import main as full_main

    full_main()


if __name__ == "__main__":
    main()
