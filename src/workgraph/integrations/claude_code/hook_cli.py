"""``workgraph claude hook`` - the command Claude Code runs for lifecycle hooks.

Contract:

* Reads the documented hook JSON from stdin and **always exits 0** with no stdout,
  so instrumentation never blocks, alters or adds context to a Claude Code session.
* Imports as little as possible and uses a short database busy timeout.
* If immediate ingestion fails for any reason (database locked, not initialised,
  schema mismatch, bug), the redacted payload is written atomically to the spool
  directory and replayed later (``workgraph claude replay`` or the worker).
"""

from __future__ import annotations

import json
import os
import sys
import uuid
from datetime import UTC, datetime
from typing import Any

MAX_INPUT_BYTES = 8 * 1024 * 1024


def _spool(paths: Any, record: dict[str, Any]) -> None:
    from workgraph.core.fsutil import atomic_write_text

    name = f"{record['received_at'].replace(':', '').replace('-', '')}-{uuid.uuid4().hex[:12]}.json"
    atomic_write_text(paths.spool_dir / name, json.dumps(record, default=str))


def run_hook(argv: list[str]) -> int:
    debug = "--debug" in argv
    received_at = datetime.now(UTC).isoformat()
    try:
        raw = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
    except Exception:
        return 0
    try:
        from workgraph.config.paths import AppPaths

        paths = AppPaths.resolve()
        paths.spool_dir.mkdir(parents=True, exist_ok=True)
    except Exception:
        return 0
    env_context = {
        "attempt_id": os.environ.get("WORKGRAPH_ATTEMPT_ID"),
        "task_id": os.environ.get("WORKGRAPH_TASK_ID"),
        "workspace": os.environ.get("WORKGRAPH_WORKSPACE"),
    }
    try:
        if len(raw) > MAX_INPUT_BYTES:
            payload: Any = {"hook_event_name": "Oversized", "size": len(raw)}
        else:
            payload = json.loads(raw.decode("utf-8", errors="replace") or "{}")
    except json.JSONDecodeError as exc:
        payload = {
            "hook_event_name": "Invalid",
            "error": str(exc),
            "excerpt": raw[:200].decode("utf-8", "replace"),
        }
    record = {"received_at": received_at, "env": env_context, "payload": payload}
    try:
        from workgraph.integrations.claude_code.ingest import ingest_direct

        ingest_direct(paths, record)
    except Exception as exc:
        try:
            from workgraph.security.redaction import redact

            record["payload"] = redact(record["payload"], max_chars=20_000)
            record["spool_reason"] = f"{type(exc).__name__}: {exc}"[:500]
            _spool(paths, record)
        except Exception:
            pass
        if debug:
            print(f"workgraph hook spooled: {exc}", file=sys.stderr)
    return 0
