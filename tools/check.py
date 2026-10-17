"""Unified quality gate: `uv run tools/check.py` (add --fast to skip slow tests, --no-web to skip the frontend).

Runs, in order: ruff lint, ruff format check, ty, pytest with coverage (80% gate) and,
when the frontend dependencies are installed, pnpm typecheck / test / build.
Exits non-zero on the first failing step.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def step(name: str, argv: list[str], cwd: Path = ROOT) -> None:
    print(f"\n==> {name}: {' '.join(argv)}", flush=True)
    started = time.monotonic()
    result = subprocess.run(argv, cwd=cwd, check=False)
    elapsed = time.monotonic() - started
    if result.returncode != 0:
        print(f"FAILED: {name} (exit {result.returncode}, {elapsed:.1f}s)")
        raise SystemExit(result.returncode)
    print(f"ok: {name} ({elapsed:.1f}s)")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fast", action="store_true", help="skip tests marked slow/e2e and the coverage gate"
    )
    parser.add_argument("--no-web", action="store_true", help="skip frontend checks")
    args = parser.parse_args()
    uv = shutil.which("uv") or "uv"
    step("ruff lint", [uv, "run", "ruff", "check", "."])
    step("ruff format", [uv, "run", "ruff", "format", "--check", "."])
    step("ty", [uv, "run", "ty", "check"])
    if args.fast:
        step("pytest (fast)", [uv, "run", "pytest", "-q", "-m", "not slow and not e2e"])
    else:
        step(
            "pytest + coverage",
            [uv, "run", "pytest", "-q", "--cov=workgraph", "--cov-report=term", "--cov-fail-under=80"],
        )
    web = ROOT / "web"
    pnpm = shutil.which("pnpm")
    if args.no_web:
        print("\n(skipping frontend checks)")
    elif pnpm and (web / "node_modules").exists():
        step("web typecheck", [pnpm, "typecheck"], cwd=web)
        step("web unit tests", [pnpm, "test"], cwd=web)
        step("web build", [pnpm, "build"], cwd=web)
    else:
        print("\n(frontend dependencies not installed; run `pnpm --dir web install` to include web checks)")
    print("\nall checks passed")


if __name__ == "__main__":
    sys.exit(main())
