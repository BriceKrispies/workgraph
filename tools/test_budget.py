#!/usr/bin/env python3
"""Run `cargo test --workspace` under the 120 s test budget and report where the time went.

    python tools/test_budget.py [--budget SECS]

The test binaries are built first in an unbudgeted `cargo test --workspace --no-run`
step, so the budget covers only running them. Then `cargo test --workspace` runs under
the budget. Past the budget the run is killed with its process tree and the script exits
1, printing the binaries that were running and the slowest ones so far. A budget above
120 s is refused: the ceiling is not configurable upward.

Per-binary wall times come from cargo's `Running` and `Doc-tests` headers, which it
prints before each target runs. A binary's time is the gap to the next header.
"""

from __future__ import annotations

import argparse
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check import ROOT, kill_tree  # noqa: E402  (reuses the process-tree killer)

CEILING_SECS = 120
HEADER = re.compile(r"^\s+(Running|Doc-tests)\s+(.*)$")


def run_budgeted(argv: list[str], budget: float) -> tuple[int, list[tuple[float, str]], bool]:
    """Run `argv` under `budget` seconds. Returns (exit code, per-target durations, timed_out)."""
    proc = subprocess.Popen(
        argv,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        start_new_session=sys.platform != "win32",
        encoding="utf-8",
        errors="replace",
    )
    lines: queue.Queue[str | None] = queue.Queue()

    def pump() -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()

    started = time.monotonic()
    deadline = started + budget
    headers: list[tuple[float, str]] = []
    while True:
        remaining = deadline - time.monotonic()
        try:
            line = lines.get(timeout=max(remaining, 0.0))
        except queue.Empty:
            kill_tree(proc)
            headers.append((time.monotonic() - started, "<killed at budget>"))
            return -1, _durations(headers), True
        if line is None:
            break
        sys.stdout.write(line)
        sys.stdout.flush()
        match = HEADER.match(line)
        if match:
            headers.append((time.monotonic() - started, match.group(2).strip()))
    code = proc.wait()
    headers.append((time.monotonic() - started, "<end>"))
    return code, _durations(headers), False


def _durations(headers: list[tuple[float, str]]) -> list[tuple[float, str]]:
    """Turn (start time, label) headers into (duration, label) rows, dropping the end marker."""
    rows = []
    for (start, label), (end, _) in zip(headers, headers[1:]):
        rows.append((end - start, label))
    return rows


def report(rows: list[tuple[float, str]], total: float) -> None:
    print(f"\nslowest test targets (wall time, of {total:.1f} s total):")
    for seconds, label in sorted(rows, reverse=True)[:10]:
        print(f"  {seconds:7.1f} s  {label}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--budget", type=float, default=CEILING_SECS, help="seconds (at most 120)")
    args = parser.parse_args()
    if not 0 < args.budget <= CEILING_SECS:
        parser.error(f"--budget must be in (0, {CEILING_SECS}]; the test budget cannot be raised")

    cargo = shutil.which("cargo") or "cargo"
    print("==> cargo test --workspace --no-run (unbudgeted build)", flush=True)
    build = subprocess.run([cargo, "test", "--workspace", "--no-run"], cwd=ROOT, check=False)
    if build.returncode != 0:
        print("FAILED: build of the test binaries")
        return build.returncode

    print(f"\n==> cargo test --workspace (budget {args.budget:g} s)", flush=True)
    started = time.monotonic()
    code, rows, timed_out = run_budgeted([cargo, "test", "--workspace"], args.budget)
    total = time.monotonic() - started
    report(rows, total)
    if timed_out:
        print(f"\nFAILED: test budget of {args.budget:g} s exceeded (cargo test --workspace)")
        return 1
    if code != 0:
        print(f"\nFAILED: cargo test --workspace (exit {code}, {total:.1f} s)")
        return code
    print(f"\nok: cargo test --workspace within {args.budget:g} s ({total:.1f} s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
