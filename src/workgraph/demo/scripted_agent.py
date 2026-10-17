"""A deterministic command agent speaking the ``workgraph.agent.v1`` protocol.

Used by the end-to-end demonstration and the test-suite in place of a paid coding
agent. It performs *real* work: it edits files in its managed worktrees and creates
Git commits carrying a ``Task-ID`` trailer.

If a repository contains ``workgraph-demo.json`` (``{"file": ..., "content": ...}``)
the agent writes that file; otherwise it appends a function to ``--file``.

Modes:
  success   implement and commit in every writable repository
  buggy     commit an implementation that is missing the verification marker
  fail      report failure without committing
  sleep     sleep (timeout/cancellation scenarios)
  crash     exit with status 3 and no result
  noop      succeed without changes
  touch-ro  also modify read-only repositories (policy violation)
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
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "Workgraph Scripted Agent",
        "GIT_AUTHOR_EMAIL": "agent@workgraph.local",
        "GIT_COMMITTER_NAME": "Workgraph Scripted Agent",
        "GIT_COMMITTER_EMAIL": "agent@workgraph.local",
    }
    subprocess.run(["git", "-C", path, *args], check=True, capture_output=True, env=env)


def implement(repo: dict[str, str], task_key: str, mode: str, default_file: str) -> str:
    root = Path(repo["checkout_path"])
    plan_file = root / "workgraph-demo.json"
    if plan_file.exists():
        plan = json.loads(plan_file.read_text(encoding="utf-8"))
        target = root / plan["file"]
        content = plan["content"].replace("{task}", task_key)
        if mode == "buggy":
            content = content.replace("WORKGRAPH_DEMO_OK", "TODO_FINISH")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8", newline="\n")
    else:
        target = root / default_file
        previous = target.read_text(encoding="utf-8") if target.exists() else ""
        index = len(previous.splitlines())
        target.write_text(
            previous + f"def change_{index}():\n    return '{task_key}'\n", encoding="utf-8", newline="\n"
        )
    git(repo["checkout_path"], "add", "-A")
    git(
        repo["checkout_path"],
        "commit",
        "-q",
        "-m",
        f"Implement {task_key} in {repo['repository']}\n\nTask-ID: {task_key}",
    )
    return str(target.relative_to(root))


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
        print("boom: simulated crash", flush=True)  # noqa: T201 - deliberate unstructured output
        return 3
    if args.mode == "fail":
        emit(type="progress", message="tests are failing", percent=50)
        emit(type="result", status="failed", summary="could not make it work", error="simulated failure")
        return 1
    changed = []
    for repo in manifest["repositories"]:
        if repo["access"] != "read_write" and args.mode != "touch-ro":
            continue
        if args.mode == "noop":
            continue
        path = implement(repo, task_key, args.mode, args.file)
        changed.append(f"{repo['repository']}:{path}")
        emit(type="progress", message=f"committed {path} in {repo['repository']}", percent=60)
    emit(type="usage", input_tokens=1200, output_tokens=340)
    emit(type="result", status="succeeded", summary=f"changed {', '.join(changed) or 'nothing'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
