"""Stand-in for the `claude` CLI emitting documented --output-format stream-json messages (test fixture)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time


def out(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main() -> int:
    args = sys.argv[1:]
    if args == ["--version"]:
        print("2.1.999 (Claude Code)")
        return 0
    prompt = sys.stdin.read()
    session = args[args.index("--session-id") + 1] if "--session-id" in args else "resumed"
    mode = os.environ.get("FAKE_CLAUDE_MODE", "success")
    out(
        {
            "type": "system",
            "subtype": "init",
            "session_id": session,
            "model": "claude-test",
            "tools": ["Bash", "Edit"],
        }
    )
    out(
        {
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "text", "text": "Working on it"},
                    {"type": "tool_use", "id": "t1", "name": "Edit", "input": {}},
                    {"type": "tool_use", "id": "t2", "name": "Task", "input": {}},
                ]
            },
        }
    )
    out(
        {
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": True}]},
        }
    )
    if mode == "sleep":
        time.sleep(600)
    if mode == "success" and "Task-ID:" in prompt:
        key = prompt.split("Task-ID: ")[1].split("`")[0]
        with open("claude_change.py", "a", encoding="utf-8") as fh:
            fh.write("def from_claude():\n    return 1\n")
        env = {
            **os.environ,
            "GIT_AUTHOR_NAME": "c",
            "GIT_AUTHOR_EMAIL": "c@x",
            "GIT_COMMITTER_NAME": "c",
            "GIT_COMMITTER_EMAIL": "c@x",
        }
        subprocess.run(["git", "add", "-A"], check=True, env=env)
        subprocess.run(["git", "commit", "-qm", f"claude work\n\nTask-ID: {key}"], check=True, env=env)
    out(
        {
            "type": "result",
            "subtype": "success" if mode == "success" else "error_during_execution",
            "is_error": mode != "success",
            "duration_ms": 4321,
            "num_turns": 3,
            "result": "All done",
            "session_id": session,
            "total_cost_usd": 0.0123,
            "usage": {"input_tokens": 900, "output_tokens": 120, "cache_read_input_tokens": 50},
        }
    )
    return 0 if mode == "success" else 1


if __name__ == "__main__":
    raise SystemExit(main())
