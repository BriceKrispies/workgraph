"""Record raw MCP JSON-RPC transcripts from the Python server (task CF-03).

Writes two files under ``contracts/mcp/``:

* ``stdio.jsonl``  an exchange with ``workgraph mcp`` over the stdio transport
* ``http.jsonl``   an exchange with ``workgraph mcp --transport http`` (Streamable HTTP, ``/mcp/``)

Each line is one exchange, sorted-key JSON::

    {"kind": "...", "label": "...", "tool": "...", "request": {...}, "response": {...} | null}

``kind`` is ``protocol`` (initialize, listings, notifications), ``ok`` (a call expected to succeed),
``error`` (a call expected to fail: missing entity, unknown enum value, and so on), ``unknown``
(a tool name the server does not have), ``invalid`` (arguments that fail schema validation) or
``setup`` (a call that prepares state for later calls; it is recorded like any other).

Every run uses fresh temporary Workgraph homes (one per transport) seeded with the demo data, an
experiment, and a scratch Git repository. The live home is never touched. Ids and timestamps are
recorded raw; normalisation is section 3.4 of ``docs/rust-rewrite.md`` and is applied at replay.

Usage::

    uv run python tools/contracts/record_mcp.py

The script fails if the scenario does not call every tool named in ``contracts/mcp.json``.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "contracts" / "mcp"
TOOLS_SNAPSHOT = ROOT / "contracts" / "mcp.json"
PROTOCOL_VERSION = "2025-06-18"
MODEL = "claude-haiku-5-5"

EXPERIMENT_PROTOCOL = {
    "arms": [{"name": "haiku", "model": MODEL}],
    "brief_template": "leaf-v1",
    "primary_size": "source_lines",
    "bands": {"min": 1, "max": 5},
    "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 6, "max_trials": 24},
    "budget": {"trials": 40},
}

Save = dict[str, Callable[[dict[str, Any]], Any]]

# Scenario steps that did not behave as their kind promises; printed at the end of a run.
PROBLEMS: list[str] = []

RULE_NAME = "recorded manual rule"

# Labels whose success cannot be reached with the demo seed, and why. They are still recorded
# (with the server's real error) and reported as known gaps instead of failing the run.
KNOWN_GAPS: dict[str, str] = {
    "trial_start:ok": "no replay tasks: `experiment replay scan` finds no candidate commits in the demo repositories",
    "trial_grade:ok": "no started trial (see trial_start)",
    "trial_classify:ok": "no failed trial (see trial_start)",
    "experiment_fit:ok": "needs at least four graded trials with src_lines, src_files and packages",
    "experiment_predict:ok": "needs task metadata contract.predicted, which only the experiment pipeline writes",
}


def _first(sc: dict[str, Any], *paths: tuple[str, ...]) -> Any:
    """Return the first value found at any of the key paths in a structured result."""
    for path in paths:
        node: Any = sc
        for part in path:
            node = node.get(part) if isinstance(node, dict) else None
        if node is not None:
            return node
    raise KeyError(f"none of {paths} in result")


def _attempt_id(sc: dict[str, Any]) -> str:
    return _first(sc, ("attempt_id",), ("id",), ("attempt", "id"), ("attempt", "attempt_id"))


def _trial_id(sc: dict[str, Any]) -> str:
    """The first trial of an experiment; a placeholder when there is none (see KNOWN_GAPS)."""
    trials = sc.get("trials") or []
    return trials[0]["id"] if trials else "00000000-0000-0000-0000-000000000000"


def _repo_name(sc: dict[str, Any]) -> str:
    repo = sc["repositories"][0]
    return repo.get("name") or repo.get("slug")


def _key(sc: dict[str, Any]) -> str:
    return sc["key"]


class Session:
    """One MCP session over a transport. ``call`` returns the raw response."""

    def __init__(self, transport: str, home: Path, grants: list[str], scratch: Path) -> None:
        self.transport = transport
        self.home = home
        self.grants = grants
        self.scratch = scratch
        self.next_id = 0
        self.proc: subprocess.Popen[str] | None = None
        self.url = ""
        self.session_id: str | None = None
        self.token: str | None = None

    # ----------------------------------------------------------------- lifecycle

    def __enter__(self) -> Session:
        env = {**os.environ, "WORKGRAPH_HOME": str(self.home)}
        grant_args = [arg for g in self.grants for arg in ("--grant", g)]
        base = [sys.executable, "-m", "workgraph", "mcp", "--agent-name", "recorder", *grant_args]
        self.log = (self.scratch / f"{self.transport}-server.log").open("w", encoding="utf-8")
        if self.transport == "stdio":
            self.proc = subprocess.Popen(
                base,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=self.log,
                env=env,
                text=True,
                encoding="utf-8",
                bufsize=1,
            )
        else:
            port = _free_port()
            self.url = f"http://127.0.0.1:{port}/mcp/"
            self.proc = subprocess.Popen(
                [*base, "--transport", "http", "--port", str(port)],
                stdout=self.log,
                stderr=subprocess.STDOUT,
                env=env,
            )
            _wait_for_port(port, self.proc)
            token_file = _token_file(self.home)
            if token_file is not None and token_file.exists():
                self.token = token_file.read_text(encoding="utf-8").strip()
        return self

    def __exit__(self, *exc: object) -> None:
        if self.proc is not None:
            if self.proc.stdin is not None:
                self.proc.stdin.close()
            try:
                self.proc.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.log.close()

    # ----------------------------------------------------------------- transport

    def send(self, message: dict[str, Any]) -> dict[str, Any] | None:
        """Send one JSON-RPC message; return the response, or None for a notification."""
        if self.transport == "stdio":
            return self._send_stdio(message)
        return self._send_http(message)

    def _send_stdio(self, message: dict[str, Any]) -> dict[str, Any] | None:
        assert self.proc is not None and self.proc.stdin and self.proc.stdout
        self.proc.stdin.write(json.dumps(message, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()
        if "id" not in message:
            return None
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("stdio server closed its stdout")
        return json.loads(line)

    def _send_http(self, message: dict[str, Any]) -> dict[str, Any] | None:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        data = json.dumps(message, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(self.url, data=data, headers=headers, method="POST")  # noqa: S310
        try:
            # The URL is the loopback server this script started itself.
            with urllib.request.urlopen(request, timeout=300) as response:  # noqa: S310
                body = response.read().decode("utf-8")
                session = response.headers.get("mcp-session-id")
                if session:
                    self.session_id = session
                content_type = response.headers.get("content-type", "")
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8")
            content_type = exc.headers.get("content-type", "")
            if "id" not in message:
                return None
            return _parse_http_body(body, content_type, message["id"])
        if "id" not in message:
            return None
        return _parse_http_body(body, content_type, message["id"])

    # ----------------------------------------------------------------- requests

    def exchange(
        self, method: str, params: dict[str, Any] | None = None, *, notify: bool = False
    ) -> tuple[dict[str, Any], dict[str, Any] | None]:
        request: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            request["params"] = params
        if not notify:
            self.next_id += 1
            request["id"] = self.next_id
        return request, self.send(request)


def _parse_http_body(body: str, content_type: str, request_id: Any) -> dict[str, Any] | None:
    if not body.strip():
        return None
    if "text/event-stream" in content_type:
        found = None
        for line in body.splitlines():
            if line.startswith("data:"):
                payload = json.loads(line[len("data:") :].strip())
                if payload.get("id") == request_id:
                    found = payload
        return found
    return json.loads(body)


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _wait_for_port(port: int, proc: subprocess.Popen[str] | subprocess.Popen[bytes]) -> None:
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"MCP HTTP server exited with {proc.returncode}")
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            if sock.connect_ex(("127.0.0.1", port)) == 0:
                return
        time.sleep(0.2)
    raise RuntimeError("MCP HTTP server did not start listening")


def _token_file(home: Path) -> Path | None:
    from workgraph.config.paths import AppPaths

    return AppPaths.resolve(home).api_token_file


# --------------------------------------------------------------------- seeding


def _workgraph(home: Path, *args: str) -> str:
    env = {**os.environ, "WORKGRAPH_HOME": str(home)}
    proc = subprocess.run(
        [sys.executable, "-m", "workgraph", *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        env=env,
        check=True,
        timeout=900,
    )
    return proc.stdout


def _git(path: Path, *args: str) -> str:
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "recorder",
        "GIT_AUTHOR_EMAIL": "recorder@example.invalid",
        "GIT_COMMITTER_NAME": "recorder",
        "GIT_COMMITTER_EMAIL": "recorder@example.invalid",
    }
    proc = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, text=True, env=env, check=True
    )
    return proc.stdout.strip()


def seed(home: Path, scratch: Path) -> dict[str, str]:
    """Populate ``home`` with demo data, an experiment and a scratch repository."""
    home.mkdir(parents=True, exist_ok=True)
    scratch.mkdir(parents=True, exist_ok=True)
    _workgraph(home, "demo", "seed", "--workspace", "demo")
    protocol = scratch / "protocol.json"
    protocol.write_text(json.dumps(EXPERIMENT_PROTOCOL), encoding="utf-8")
    _workgraph(
        home,
        "experiment",
        "create",
        "cli-exp",
        "--title",
        "CLI experiment",
        "--question",
        "Does it?",
        "--protocol",
        f"@{protocol}",
        "--workspace",
        "demo",
    )
    _workgraph(home, "experiment", "freeze", "cli-exp", "--workspace", "demo")

    rule = scratch / "rule.json"
    rule.write_text(
        json.dumps(
            {
                "name": RULE_NAME,
                "trigger": {"type": "manual"},
                "conditions": {"field": "task.priority", "op": "eq", "value": "critical"},
                "actions": [{"type": "add_tags", "tags": ["triage"]}],
            }
        ),
        encoding="utf-8",
    )
    _workgraph(home, "automation", "create", f"@{rule}", "--workspace", "demo")

    extra = scratch / "extra-repo"
    extra.mkdir()
    _git(extra, "init", "-q")
    (extra / "README.md").write_text("recorded scratch repository\n", encoding="utf-8")
    _git(extra, "add", "README.md")
    _git(extra, "commit", "-q", "-m", "Initial commit")
    head = _git(extra, "rev-parse", "HEAD")

    not_repo = scratch / "not-a-repo"
    not_repo.mkdir()
    return {"extra_path": str(extra), "extra_head": head, "not_repo_path": str(not_repo), "rule": RULE_NAME}


# -------------------------------------------------------------------- scenario

Step = dict[str, Any]


def _step(
    kind: str,
    label: str,
    tool: str | None,
    args: dict[str, Any] | None = None,
    *,
    save: Save | None = None,
    method: str | None = None,
    params: dict[str, Any] | None = None,
) -> Step:
    return {
        "kind": kind,
        "label": label,
        "tool": tool,
        "args": args,
        "save": save or {},
        "method": method,
        "params": params,
    }


def _pair(tool: str, ok: dict[str, Any], err: dict[str, Any], save: Save | None = None) -> list[Step]:
    return [
        _step("ok", f"{tool}:ok", tool, ok, save=save),
        _step("error", f"{tool}:error", tool, err),
    ]


def _setup_task(label: str, title: str, save_key: str, **extra: Any) -> Step:
    args = {"title": title, "ready": True, **extra}
    return _step("setup", label, "task_create", args, save={save_key: _key})


def scenario() -> list[Step]:
    steps: list[Step] = []
    steps += [
        _step(
            "protocol",
            "initialize",
            None,
            method="initialize",
            params={
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "record_mcp", "version": "1"},
            },
        ),
        _step("protocol", "notifications/initialized", None, method="notifications/initialized"),
        _step("protocol", "tools/list", None, method="tools/list"),
        _step("protocol", "resources/list", None, method="resources/list"),
        _step("protocol", "resources/templates/list", None, method="resources/templates/list"),
        _step(
            "protocol",
            "resources/read:catalog",
            None,
            method="resources/read",
            params={"uri": "workgraph://analytics/catalog"},
        ),
    ]

    # workspaces and projects
    # A list as the arguments is not an object, so the call fails at the protocol level.
    steps += _pair("workspace_list", {}, [])
    steps += _pair("workspace_get", {}, {"workspace": "no-such-workspace"})
    steps += _pair("project_list", {}, {"workspace": "no-such-workspace"})
    steps += _pair("project_get", {"project": "platform"}, {"project": "no-such-project"})

    # repositories
    steps += _pair("repository_list", {}, {"project": "no-such-project"}, save={"repo": _repo_name})
    steps += _pair("repository_get", {"repository": "$repo"}, {"repository": "no-such-repo"})
    steps += _pair(
        "repository_register",
        {"path": "$extra_path", "name": "mcp-extra", "read_only": True},
        {"path": "$not_repo_path", "name": "mcp-broken"},
    )
    steps += _pair("repository_status", {"repository": "mcp-extra"}, {"repository": "no-such-repo"})

    # stacks on the demo umbrella task
    steps += _pair("stack_create", {"parent": "DEMO-8"}, {"parent": "DEMO-999"})
    steps += _pair("stack_get", {"parent": "DEMO-8"}, {"parent": "DEMO-999"})
    steps += _pair("stack_history", {"parent": "DEMO-8"}, {"parent": "DEMO-999"})
    steps += _pair("stack_list", {}, {"workspace": "no-such-workspace"})
    steps.append(
        _step(
            "setup",
            "setup:stack-child",
            "task_create",
            {"title": "Stack child", "ready": True, "parent": "DEMO-8"},
            save={"T2": _key},
        )
    )
    steps += _pair(
        "stack_add_item", {"parent": "DEMO-8", "task": "$T2"}, {"parent": "DEMO-8", "task": "DEMO-999"}
    )
    steps += _pair(
        "stack_remove_item", {"parent": "DEMO-8", "task": "$T2"}, {"parent": "DEMO-8", "task": "DEMO-999"}
    )
    steps += _pair("stack_integrate", {"parent": "DEMO-8"}, {"parent": "DEMO-999"})
    steps += _pair("stack_verify", {"parent": "DEMO-8"}, {"parent": "DEMO-999"})

    # experiments
    steps += _pair("experiment_list", {}, {"status": "bogus"})
    steps += _pair("experiment_next", {"experiment": "cli-exp"}, {"experiment": "no-such-experiment"})
    steps += _pair(
        "experiment_get",
        {"experiment": "cli-exp"},
        {"experiment": "no-such-experiment"},
        save={"trial": _trial_id},
    )
    steps += _pair("trial_start", {"trial": "$trial"}, {"trial": "no-such-trial"})
    steps += _pair(
        "trial_grade", {"trial": "$trial", "timeout": 60}, {"trial": "no-such-trial", "timeout": 60}
    )
    steps += _pair(
        "trial_classify",
        {"trial": "$trial", "cause": "other", "source": "mechanical", "notes": "recorded"},
        {"trial": "no-such-trial", "cause": "other", "source": "mechanical"},
    )
    steps += _pair("experiment_report", {"experiment": "cli-exp"}, {"experiment": "no-such-experiment"})
    steps += _pair("experiment_fit", {"model": MODEL, "samples": 50, "seed": 1}, {"model": "no-such-model"})
    steps += _pair(
        "experiment_predict",
        {"model": MODEL, "task": "DEMO-1", "src_lines": 40},
        {"model": MODEL, "task": "DEMO-999"},
    )

    # tasks: creation and reads
    steps += _pair(
        "task_create",
        {"title": "MCP recorded task", "description": "Recorded for CF-03", "ready": True, "tags": ["mcp"]},
        {"title": "Recorded task", "parent": "DEMO-999"},
        save={"T1": _key},
    )
    steps += _pair("task_get", {"task": "DEMO-1"}, {"task": "DEMO-999"})
    steps += _pair(
        "task_contract_check",
        {"task": "DEMO-1", "model": MODEL, "target": 0.8},
        {"task": "DEMO-999", "model": MODEL},
    )
    steps += _pair(
        "task_search",
        {"query": "token", "mode": "lexical", "limit": 5},
        {"query": "token", "mode": "bogus", "limit": 5},
    )
    steps += _pair("task_list", {"statuses": ["accepted"], "limit": 5}, {"statuses": ["bogus"]})
    steps += _pair(
        "task_update",
        {"task": "$T1", "title": "MCP recorded task (renamed)"},
        {"task": "DEMO-999", "title": "x"},
    )
    steps += _pair("task_get_ready", {"limit": 5}, {"limit": 0})
    steps += _pair(
        "task_promote_ready", {"dry_run": True, "all": True}, {"project": "no-such-project", "all": True}
    )

    # dependencies and relations on fresh tasks
    steps.append(_setup_task("setup:dep-task", "Dependent task", "T4"))
    steps.append(_setup_task("setup:dep-target", "Dependency target", "T5"))
    steps += _pair(
        "task_add_dependency", {"task": "$T4", "depends_on": "$T5"}, {"task": "$T4", "depends_on": "DEMO-999"}
    )
    steps += _pair(
        "task_remove_dependency",
        {"task": "$T4", "depends_on": "$T5"},
        {"task": "$T4", "depends_on": "DEMO-999"},
    )
    steps += _pair(
        "task_add_relation",
        {"task": "$T4", "target": "$T5", "relation_type": "relates_to"},
        {"task": "$T4", "target": "DEMO-999", "relation_type": "relates_to"},
    )
    steps += _pair(
        "task_add_comment", {"task": "$T1", "body": "Recorded comment"}, {"task": "DEMO-999", "body": "x"}
    )
    steps += _pair("task_get_history", {"task": "DEMO-1", "limit": 10}, {"task": "DEMO-999", "limit": 10})
    steps += _pair(
        "task_get_attempts",
        {"task": "DEMO-1", "limit": 10, "offset": 0},
        {"task": "DEMO-999", "limit": 10, "offset": 0},
    )

    # claims, releases and completion
    steps += _pair(
        "task_claim", {"task": "$T1", "lease_minutes": 30}, {"task": "DEMO-999", "lease_minutes": 30}
    )
    steps.append(_setup_task("setup:release-task", "Release task", "T3"))
    steps.append(_step("setup", "setup:release-claim", "task_claim", {"task": "$T3", "lease_minutes": 30}))
    steps += _pair("task_release", {"task": "$T3", "reason": "recording"}, {"task": "DEMO-999"})
    steps.append(_setup_task("setup:complete-task", "Completion task", "T6"))
    steps.append(_step("setup", "setup:complete-claim", "task_claim", {"task": "$T6", "lease_minutes": 30}))
    steps += _pair(
        "task_complete", {"task": "$T6", "summary": "Recorded"}, {"task": "DEMO-999", "summary": "x"}
    )

    # attempts and progress on the claimed task T1
    steps += _pair(
        "attempt_create", {"task": "$T1", "model": MODEL}, {"task": "DEMO-999"}, save={"A1": _attempt_id}
    )
    steps += _pair("attempt_get", {"attempt_id": "$A1"}, {"attempt_id": "no-such-attempt"})
    steps += _pair("attempt_list", {"limit": 10, "offset": 0}, {"limit": 0})
    steps += _pair(
        "task_report_progress",
        {"task": "$T1", "message": "halfway", "percent": 50, "attempt_id": "$A1"},
        {"task": "DEMO-999", "message": "x"},
    )
    steps += _pair(
        "task_report_result",
        {
            "task": "$T1",
            "outcome": "succeeded",
            "summary": "done",
            "attempt_id": "$A1",
            "input_tokens": 10,
            "output_tokens": 5,
        },
        {"task": "DEMO-999", "outcome": "succeeded"},
    )

    # attempts prepared and submitted through the external-attempt flow
    steps.append(_setup_task("setup:external-task", "External attempt task", "T9"))
    steps += _pair(
        "attempt_start",
        {"task": "$T9", "adapter": "claude_code_subagent", "isolation": "in_place"},
        {"task": "DEMO-999", "adapter": "claude_code_subagent"},
        save={"A2": _attempt_id},
    )
    steps += _pair(
        "attempt_submit",
        {"attempt": "$A2", "status": "failed", "summary": "recorded", "error": "recorded"},
        {"attempt": "no-such-attempt", "status": "failed"},
    )

    # managed runs
    steps.append(_setup_task("setup:run-task", "Managed run task", "T10"))
    steps += _pair(
        "run_start",
        {"task": "$T10", "agent": "scripted", "model": MODEL},
        {"task": "DEMO-999"},
        save={"R1": _attempt_id},
    )
    steps += _pair(
        "run_get", {"attempt_id": "$R1", "events": 5}, {"attempt_id": "no-such-attempt", "events": 5}
    )
    steps += _pair(
        "run_cancel",
        {"attempt_id": "$R1", "reason": "recording"},
        {"attempt_id": "no-such-attempt", "reason": "x"},
    )

    # decisions
    steps.append(_setup_task("setup:decision-task", "Decision task", "T7"))
    steps += _pair(
        "task_request_decision",
        {
            "task": "$T7",
            "question": "Which way?",
            "options": [{"label": "Yes"}, {"label": "No", "recommended": True}],
        },
        {"task": "DEMO-999", "question": "x"},
    )
    steps += _pair("decision_list", {"status": "all", "limit": 10}, {"status": "bogus"})
    steps += _pair("task_decisions", {"task": "$T7"}, {"task": "DEMO-999"})

    # repositories linked to tasks, commits and git reads
    steps += _pair(
        "task_link_repository",
        {"task": "$T1", "repository": "mcp-extra", "read_only": True},
        {"task": "DEMO-999", "repository": "mcp-extra"},
    )
    steps += _pair(
        "task_link_commit",
        {"task": "$T1", "sha": "$extra_head", "repository": "mcp-extra", "note": "recorded"},
        {"task": "DEMO-999", "sha": "$extra_head"},
    )
    steps += _pair(
        "git_get_changes",
        {"task": "DEMO-1", "min_confidence": 0.5},
        {"task": "DEMO-999", "min_confidence": 0.5},
    )
    steps += _pair(
        "git_get_task_diff", {"task": "DEMO-1", "max_bytes": 4000}, {"task": "DEMO-999", "max_bytes": 4000}
    )

    # classification
    steps += _pair("classification_get", {"task": "DEMO-1"}, {"task": "DEMO-999"})
    steps += _pair("classification_request", {"task": "DEMO-1"}, {"task": "DEMO-999"})

    # analytics
    steps += _pair(
        "analytics_query", {"metric": "tasks_created", "period": "week"}, {"metric": "no_such_metric"}
    )
    steps += _pair("analytics_report", {"report": "overview"}, {"report": "no_such_report"})
    steps += _pair("analytics_subagents", {"period": "week"}, {"period": "century"})
    steps += _pair("analytics_agent_time", {"by": "class"}, {"by": "nonsense"})
    steps += _pair("analytics_bottlenecks", {"top": 5}, {"top": 0})

    # automation
    steps += _pair("automation_list", {}, {"workspace": "no-such-workspace"})
    steps += _pair("automation_trigger", {"rule": "$rule"}, {"rule": "no-such-rule"})

    # breakdown import, dry run
    breakdown = "## Plan\n\n**CF-99 Recorded item**\nTouches: none.\nAccept: none.\nDeps: none.\n"
    steps += _pair(
        "task_import_breakdown",
        {
            "markdown": breakdown,
            "parent": "DEMO-8",
            "section": "Plan",
            "source": "contracts/record_mcp",
            "dry_run": True,
        },
        {
            "markdown": breakdown,
            "parent": "DEMO-999",
            "section": "Plan",
            "source": "contracts/record_mcp",
            "dry_run": True,
        },
    )

    # the protocol-level failures
    steps.append(_step("unknown", "tools/call:unknown", "no_such_tool", {}))
    steps.append(_step("invalid", "tools/call:invalid-args", "task_get", {"task": 5}))
    return steps


# ------------------------------------------------------------------ recording


def run_transport(transport: str, scratch: Path, grants: list[str]) -> list[dict[str, Any]]:
    home = scratch / f"home-{transport}"
    store: dict[str, Any] = seed(home, scratch / f"scratch-{transport}")
    records: list[dict[str, Any]] = []
    steps = scenario()
    with Session(transport, home, grants, scratch) as session:
        for seq, step in enumerate(steps, start=1):
            kind = step["kind"]
            if step["method"] is not None:
                notify = step["method"].startswith("notifications/")
                request, response = session.exchange(step["method"], step["params"], notify=notify)
                records.append(_record(seq, kind, step["label"], None, request, response))
                continue
            args = step["args"] if isinstance(step["args"], list) else _fill(step["args"], store)
            request, response = session.exchange("tools/call", {"name": step["tool"], "arguments": args})
            result = (response or {}).get("result") or {}
            records.append(_record(seq, kind, step["label"], step["tool"], request, response))
            failed = bool(result.get("isError")) or "error" in (response or {})
            if kind in ("ok", "setup") and failed:
                _problem(transport, step["label"], f"{json.dumps(response, ensure_ascii=False)[:600]}")
            if kind in ("ok", "setup") and not failed:
                structured = result.get("structuredContent") or {}
                for name, extract in step["save"].items():
                    try:
                        store[name] = extract(structured)
                    except (KeyError, IndexError, TypeError) as exc:
                        _problem(transport, step["label"], f"capture {name} failed ({exc})")
            if kind == "error" and not failed:
                _problem(transport, step["label"], "expected an error, got success")
    return records


def _problem(transport: str, label: str, detail: str) -> None:
    """Record a step that missed its kind. Known gaps are not failures."""
    if label in KNOWN_GAPS:
        PROBLEMS.append(f"known gap {transport} {label}: {KNOWN_GAPS[label]}")
    else:
        PROBLEMS.append(f"{transport} {label}: {detail}")


def _fill(value: Any, store: dict[str, Any]) -> Any:
    if isinstance(value, str) and value.startswith("$") and value[1:] in store:
        return store[value[1:]]
    if isinstance(value, dict):
        return {k: _fill(v, store) for k, v in value.items()}
    if isinstance(value, list):
        return [_fill(v, store) for v in value]
    return value


def _record(
    seq: int,
    kind: str,
    label: str,
    tool: str | None,
    request: dict[str, Any],
    response: dict[str, Any] | None,
) -> dict[str, Any]:
    """One exchange. ``outcome`` says what the server answered: ``success``, ``error`` or ``none``."""
    if response is None:
        outcome = "none"
    elif "error" in response or (response.get("result") or {}).get("isError"):
        outcome = "error"
    else:
        outcome = "success"
    return {
        "seq": seq,
        "kind": kind,
        "label": label,
        "tool": tool,
        "outcome": outcome,
        "request": request,
        "response": response,
    }


def _check_coverage(records: list[dict[str, Any]]) -> None:
    snapshot = json.loads(TOOLS_SNAPSHOT.read_text(encoding="utf-8"))
    expected = {tool["name"] for tool in snapshot["tools"]}
    ok_tools = {r["tool"] for r in records if r["kind"] == "ok"}
    err_tools = {r["tool"] for r in records if r["kind"] == "error"}
    missing = sorted((expected - ok_tools) | (expected - err_tools))
    if missing:
        raise RuntimeError(f"scenario does not cover tools: {missing}")


def _write(path: Path, records: list[dict[str, Any]]) -> None:
    lines = [json.dumps(record, sort_keys=True, ensure_ascii=False) for record in records]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main() -> int:
    from workgraph.security.permissions import Permission

    grants = [permission.value for permission in Permission]
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="record-mcp-"))
    try:
        for transport in ("stdio", "http"):
            records = run_transport(transport, scratch, grants)
            if transport == "stdio":
                _check_coverage(records)
            _write(OUT_DIR / f"{transport}.jsonl", records)
            print(f"{transport}: {len(records)} exchanges written to {OUT_DIR / (transport + '.jsonl')}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    for problem in PROBLEMS:
        print(f"NOTE {problem}", file=sys.stderr)
    # Stdio is the transcript set that must meet the scenario; HTTP problems are reported only.
    return 1 if any(problem.startswith("stdio ") for problem in PROBLEMS) else 0


if __name__ == "__main__":
    sys.exit(main())
