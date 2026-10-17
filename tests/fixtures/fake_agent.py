"""Deterministic command agent speaking the workgraph.agent.v1 protocol (test fixture).

Modes:
  success   - edit a file in every writable repository, commit with a Task-ID trailer
  fail      - report failure without committing
  sleep     - sleep for a long time (for timeout/cancellation tests)
  crash     - exit with code 3 and no result line
  noop      - succeed without changes
  touch-ro  - also modify read-only repositories (policy violation)
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def emit(**message: object) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def git(path: str, *args: str) -> None:
    env = {**os.environ, "GIT_AUTHOR_NAME": "Fake Agent", "GIT_AUTHOR_EMAIL": "agent@example.com",
           "GIT_COMMITTER_NAME": "Fake Agent", "GIT_COMMITTER_EMAIL": "agent@example.com"}
    subprocess.run(["git", "-C", path, *args], check=True, capture_output=True, env=env)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", default="success")
    parser.add_argument("--file", default="agent_change.py")
    parser.add_argument("manifest")
    args = parser.parse_args()
    request = json.loads(sys.stdin.read() or "{}")
    manifest = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    task_key = request["task"]["key"]
    emit(type="progress", message=f"starting {task_key}", percent=5)
    if args.mode == "sleep":
        time.sleep(600)
    if args.mode == "crash":
        print("boom: simulated crash", flush=True)
        return 3
    if args.mode == "fail":
        emit(type="progress", message="tests are failing", percent=50)
        emit(type="result", status="failed", summary="could not make it work", error="simulated failure")
        return 1
    changed = []
    for repo in manifest["repositories"]:
        writable = repo["access"] == "read_write"
        if not writable and args.mode != "touch-ro":
            continue
        if args.mode == "noop":
            continue
        target = Path(repo["checkout_path"]) / args.file
        previous = target.read_text(encoding="utf-8") if target.exists() else ""
        target.write_text(previous + f"def change_{len(previous.splitlines())}():\n    return '{task_key}'\n",
                          encoding="utf-8")
        git(repo["checkout_path"], "add", "-A")
        git(repo["checkout_path"], "commit", "-q", "-m", f"Implement {task_key} in {repo['repository']}\n\nTask-ID: {task_key}")
        changed.append(repo["repository"])
        emit(type="progress", message=f"committed in {repo['repository']}", percent=50)
    emit(type="usage", input_tokens=1200, output_tokens=340)
    emit(type="result", status="succeeded", summary=f"changed {', '.join(changed) or 'nothing'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
