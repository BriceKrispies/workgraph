"""Unified quality gate: `uv run tools/check.py` (add --no-web to skip the frontend).

Runs, in order: ruff lint, ruff format check, ty, the Rust steps (cargo fmt check, clippy with
warnings denied, `cargo purity`) and, when the frontend dependencies are installed, bun run
typecheck / test / build. Add --tests to also run the Rust tests (`cargo test --workspace`) and the
conformance corpus against the Rust binaries; only
stack verification passes it, since per-change runs stay cheap.

Every step that runs tests has a hard wall-clock budget of TEST_BUDGET_SECS (120 s). A test run
that outlives it is killed with its process tree and fails as "test budget of 120 s exceeded
(<step>)". Test binaries are built in their own unbudgeted step first, so the budget only covers
running them. A check that does not fit the budget is cut from this gate, never given more time.

Exits non-zero on the first failing step.
"""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The wall-clock budget of one test run. It is a ceiling, not a default to raise.
TEST_BUDGET_SECS = 120


def step(name: str, argv: list[str], cwd: Path = ROOT) -> None:
    """Run an ordinary (non-test) step; it has no time budget."""
    print(f"\n==> {name}: {' '.join(argv)}", flush=True)
    started = time.monotonic()
    result = subprocess.run(argv, cwd=cwd, check=False)
    elapsed = time.monotonic() - started
    if result.returncode != 0:
        print(f"FAILED: {name} (exit {result.returncode}, {elapsed:.1f}s)")
        raise SystemExit(result.returncode)
    print(f"ok: {name} ({elapsed:.1f}s)")


def kill_tree(proc: subprocess.Popen[bytes]) -> None:
    """Kill a process and everything it started. A plain kill leaves children behind on Windows."""
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    else:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    proc.kill()
    proc.wait()


def test_step(name: str, argv: list[str], cwd: Path = ROOT) -> None:
    """Run a test step under the test budget. Past TEST_BUDGET_SECS it is killed and fails."""
    print(f"\n==> {name}: {' '.join(argv)} (budget {TEST_BUDGET_SECS} s)", flush=True)
    started = time.monotonic()
    proc = subprocess.Popen(argv, cwd=cwd, start_new_session=os.name != "nt")
    try:
        code = proc.wait(timeout=TEST_BUDGET_SECS)
    except subprocess.TimeoutExpired:
        kill_tree(proc)
        print(f"FAILED: test budget of {TEST_BUDGET_SECS} s exceeded ({name})")
        raise SystemExit(1) from None
    elapsed = time.monotonic() - started
    if code != 0:
        print(f"FAILED: {name} (exit {code}, {elapsed:.1f}s)")
        raise SystemExit(code)
    print(f"ok: {name} ({elapsed:.1f}s)")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-web", action="store_true", help="skip frontend checks")
    parser.add_argument(
        "--tests",
        action="store_true",
        help="also run `cargo test --workspace` (used by stack verification only)",
    )
    args = parser.parse_args()
    uv = shutil.which("uv") or "uv"
    cargo = shutil.which("cargo") or "cargo"
    step("ruff lint", [uv, "run", "ruff", "check", "."])
    step("ruff format", [uv, "run", "ruff", "format", "--check", "."])
    step("ty", [uv, "run", "ty", "check"])
    step("cargo fmt", [cargo, "fmt", "--all", "--check"])
    step("cargo clippy", [cargo, "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
    step("cargo purity", [cargo, "purity"])
    if args.tests:
        step("cargo test build", [cargo, "test", "--workspace", "--no-run"])
        test_step("cargo test", [cargo, "test", "--workspace"])
        step("cargo build bins", [cargo, "build", "--workspace", "--bins"])
        bin_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug"
        exe = ".exe" if os.name == "nt" else ""
        test_step(
            "conformance check",
            [
                str(bin_dir / f"wg-conformance{exe}"),
                "check",
                "--jobs",
                str(os.cpu_count() or 4),
                "--scenarios",
                "tests/conformance/scenarios",
                "--golden",
                "tests/conformance/golden",
                "--env",
                f"PATH={bin_dir}",
                "--rust",
                str(bin_dir / f"workgraph{exe}"),
            ],
        )
    web = ROOT / "web"
    bun = shutil.which("bun")
    if args.no_web:
        print("\n(skipping frontend checks)")
    elif bun and (web / "node_modules").exists():
        step("web typecheck", [bun, "run", "typecheck"], cwd=web)
        test_step("web unit tests", [bun, "run", "test"], cwd=web)
        step("web build", [bun, "run", "build"], cwd=web)
    else:
        print("\n(frontend dependencies not installed; run `bun install --cwd web` to include web checks)")
    print("\nall checks passed")


if __name__ == "__main__":
    sys.exit(main())
