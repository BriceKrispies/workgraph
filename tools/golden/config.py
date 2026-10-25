"""Golden vectors for the configuration model (GV-11).

Drives the Python implementation in ``src/workgraph/config/settings.py`` and writes
``golden/config/*.json``:

* ``defaults.json``  - model defaults and the saved default document;
* ``keys.json``      - every scalar key of contracts section 13.5, with a valid and an
                       invalid value each, and the contract's default next to the Python one;
* ``load.json``      - whole documents: valid, invalid, invalid TOML and v1 documents;
* ``migrate.json``   - ``config migrate`` on v1 and current files;
* ``merge.json``     - global plus workspace overrides, and ``validate`` outcomes;
* ``set.json``       - ``config set`` (value parsing and validation), mirroring
                       ``cli/commands/core.py``;
* ``writer.json``    - ``tomli_w`` output for the shapes Workgraph writes.

Run with ``uv run python tools/golden/config.py``. Output is deterministic: sorted keys,
two-space indent, LF line endings, UTF-8.
"""

from __future__ import annotations

import copy
import json
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import Any

import tomli_w
from pydantic import ValidationError

from workgraph.config.paths import AppPaths
from workgraph.config.settings import (
    CONFIG_SCHEMA_VERSION,
    ConfigStore,
    GlobalConfig,
    WorkspaceOverrides,
)
from workgraph.core.errors import ConfigError

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "config"
SLUG = "team"

# Contracts section 13.5, scalar rows: (key, contract type, contract default, allowed, valid, invalid).
# The contract default is recorded next to the Python default; differences are data, not failures.
KEY_ROWS: list[tuple[str, str, Any, str | None, Any, Any]] = [
    ("schema_version", "integer", 2, None, 2, "two"),
    ("default_workspace", "string|null", None, None, "acme", 5),
    ("server.host", "string", "127.0.0.1", None, "192.168.1.10", "   "),
    ("server.port", "integer", 8765, None, 9000, 70000),
    ("server.allow_non_loopback", "boolean", False, None, True, "maybe"),
    ("server.lan", "boolean", True, None, False, "maybe"),
    ("server.lan_address", "string|null", None, None, "192.168.1.5", 5),
    ("server.lan_networks", "array", None, None, ["10.0.0.0/8"], ["nope"]),
    ("llm.classification.provider", "string|null", None, None, "heuristic", 5),
    ("llm.classification.model", "string|null", None, None, "heuristic-v1", 5),
    ("llm.embeddings.provider", "string|null", None, None, "heuristic", 5),
    ("llm.embeddings.model", "string|null", None, None, "hash-embed-256", 5),
    ("llm.summarization.provider", "string|null", None, None, "heuristic", 5),
    ("llm.summarization.model", "string|null", None, None, "heuristic-v1", 5),
    ("llm.auto_classify", "boolean", True, None, False, "maybe"),
    ("llm.auto_embed", "boolean", True, None, False, "maybe"),
    ("execution.enabled", "boolean", False, None, True, "maybe"),
    ("execution.default_agent", "string", "claude_code", None, "command", 5),
    ("execution.max_concurrent_runs", "integer", 2, None, 4, 0),
    ("execution.max_attempts_per_task", "integer", 3, None, 5, 21),
    ("execution.auto_schedule", "boolean", False, None, True, "maybe"),
    ("execution.isolation", "string", "worktree", "worktree,in_place", "in_place", "shared"),
    ("execution.allow_push", "boolean", False, None, True, "maybe"),
    ("execution.allow_merge", "boolean", False, None, True, "maybe"),
    ("jobs.worker_concurrency", "integer", 4, None, 8, 0),
    ("jobs.gpu_concurrency", "integer", 1, None, 2, 17),
    ("jobs.per_agent_concurrency", "integer", 2, None, 3, 0),
    ("jobs.per_repository_concurrency", "integer", 1, None, 2, 65),
    ("jobs.verify_concurrency", "integer", 1, None, 2, 0),
    ("jobs.lease_seconds", "integer", 60, None, 30, 4),
    ("jobs.default_max_attempts", "integer", 5, None, 3, 51),
    ("stacks.auto_accept", "boolean", True, None, False, "maybe"),
    ("verification.auto_accept", "boolean", True, None, False, "maybe"),
    ("retention.telemetry_payload_days", "integer", 30, None, 7, 0),
    ("retention.run_log_days", "integer", 90, None, 14, 0),
    ("claude.capture_tool_payloads", "boolean", False, None, True, "maybe"),
    ("claude.max_payload_chars", "integer", 4000, None, 100, -1),
    ("automation.max_depth", "integer", 3, None, 5, 11),
]

# Contracts section 13.5 wildcard table rows: whole-table values, valid and invalid.
TABLE_ROWS: list[tuple[str, Any, Any]] = [
    (
        "llm.providers",
        {
            "ollama": {"type": "ollama", "base_url": "http://10.0.0.5:11434", "timeout_seconds": 30.0},
            "heuristic": {"type": "heuristic"},
            "lab": {
                "type": "openai_compatible",
                "base_url": "http://127.0.0.1:8080/v1",
                "max_retries": 0,
                "api_key_env": "LAB_KEY",
            },
        },
        {
            "lab": {"type": "openai_compatible"},
            "x": {"type": "nope"},
            "y": {"type": "ollama", "extra": 1},
        },
    ),
    (
        "agents",
        {
            "claude_code": {
                "type": "claude_code",
                "model": "opus",
                "allowed_tools": ["Read", "Bash(git status:*)"],
                "max_turns": 5,
                "max_budget_usd": 2.5,
            },
            "runner": {
                "type": "command",
                "executable": "/usr/bin/agent",
                "args": ["--json"],
                "env": {"MODE": "fast"},
                "supports_resume": True,
            },
        },
        {
            "runner": {"type": "command"},
            "c": {"type": "claude_code", "max_turns": 0},
            "d": {"type": "claude_code", "permission_mode": "yolo"},
        },
    ),
    (
        "trackers",
        {
            "acme": {
                "type": "jira",
                "base_url": "https://acme.atlassian.net",
                "workspace": "acme",
                "projects": ["ACME", "OPS"],
                "email_env": "JIRA_EMAIL",
                "token_env": "JIRA_TOKEN",
                "pr_fields": ["customfield_10042"],
                "cancelled_resolutions": ["Won't Do", "Duplicate"],
            }
        },
        {
            "acme": {
                "type": "jira",
                "base_url": "https://acme.atlassian.net",
                "workspace": "acme",
                "projects": ["acme"],
                "token_env": "JIRA_TOKEN",
            },
            "beta": {"type": "jira", "base_url": "https://b", "workspace": "b", "projects": ["B"]},
            "gamma": {
                "type": "jira",
                "base_url": "https://g",
                "workspace": "g",
                "projects": ["G"],
                "token_env": "T",
                "poll_interval_seconds": 10,
            },
        },
    ),
]

# Documents for load.json: (name, config.toml text).
LOAD_CASES: list[tuple[str, str]] = [
    ("empty_file", ""),
    ("current_defaults_only", "schema_version = 2\nserver = { port = 8765 }\n"),
    ("unknown_top_level_key", "bogus = 1\n"),
    ("unknown_nested_key", "[server]\nprt = 9\n"),
    ("port_zero", "[server]\nport = 0\n"),
    ("port_not_a_number", '[server]\nport = "eighty"\n'),
    ("invalid_toml", "not = [valid\n"),
    ("v1_agents_enabled_true", "[agents]\nenabled = true\n"),
    (
        "v1_agents_enabled_false_with_agent",
        "schema_version = 1\n[agents]\nenabled = false\n"
        '[agents.claude_code]\ntype = "claude_code"\nexecutable = "claude"\n',
    ),
    ("v1_without_agents_table", "schema_version = 1\n"),
    ("v1_with_server_section", "schema_version = 1\n[server]\nport = 9000\n"),
    ("multiple_errors", '[server]\nport = 0\nhost = " "\n[jobs]\nlease_seconds = 1\n'),
    ("lan_networks_invalid", '[server]\nlan_networks = ["10.0.0.0/8", "nope"]\n'),
    ("isolation_invalid", '[execution]\nisolation = "shared"\n'),
    ("provider_missing_type", '[llm.providers.x]\nbase_url = "http://x"\n'),
    ("provider_timeout_zero", '[llm.providers.ollama]\ntype = "ollama"\ntimeout_seconds = 0\n'),
    ("schema_version_3_accepted", "schema_version = 3\n"),
    ("json_style_list_in_toml", '[server]\nlan_networks = ["192.168.0.0/16"]\n'),
    (
        "tracker_jira_valid",
        'schema_version = 2\n[trackers.acme]\ntype = "jira"\nbase_url = "https://acme.atlassian.net"\n'
        'workspace = "acme"\nprojects = ["ACME"]\ntoken_env = "JIRA_TOKEN"\n',
    ),
    (
        "tracker_projects_lowercase",
        '[trackers.acme]\ntype = "jira"\nbase_url = "https://a"\nworkspace = "a"\n'
        'projects = ["acme"]\ntoken_env = "T"\n',
    ),
    (
        "execution_block_valid",
        '[execution]\nenabled = true\nmax_concurrent_runs = 4\nisolation = "in_place"\n',
    ),
]

# Migration inputs for migrate.json: name -> {relative path: text}.
MIGRATE_CASES: list[tuple[str, dict[str, str]]] = [
    ("v1_global_agents_enabled", {"config.toml": "schema_version = 1\n[agents]\nenabled = true\n"}),
    ("v2_global_unchanged", {"config.toml": "schema_version = 2\n[execution]\nenabled = true\n"}),
    ("no_files", {}),
    (
        "v1_workspace_only",
        {f"workspaces/{SLUG}.toml": "schema_version = 1\n[agents]\nenabled = true\n"},
    ),
    (
        "v1_global_and_workspace",
        {
            "config.toml": "schema_version = 1\n[agents]\nenabled = false\n",
            f"workspaces/{SLUG}.toml": "schema_version = 1\n[jobs]\nworker_concurrency = 6\n",
        },
    ),
    ("v1_global_invalid_value", {"config.toml": "schema_version = 1\n[server]\nport = 0\n"}),
    ("v1_global_invalid_toml", {"config.toml": "schema_version = 1\nnot = [valid\n"}),
]

# Merge inputs for merge.json: name -> (global text or None, workspace text or None).
MERGE_CASES: list[tuple[str, str | None, str | None]] = [
    ("workspace_execution_override", None, "[execution]\nmax_concurrent_runs = 7\n"),
    (
        "global_then_workspace_deep_merge",
        "[execution]\nenabled = true\nmax_concurrent_runs = 3\n"
        '[llm.classification]\nprovider = "heuristic"\nmodel = "heuristic-v1"\n',
        '[llm.classification]\nmodel = "other-model"\n',
    ),
    (
        "agents_deep_merge",
        '[agents.claude_code]\ntype = "claude_code"\nmax_turns = 3\n',
        '[agents.claude_code]\ntype = "claude_code"\nmax_turns = 9\n',
    ),
    ("workspace_v1_agents_enabled", None, "schema_version = 1\n[agents]\nenabled = true\n"),
    ("workspace_stacks_override", "[stacks]\nauto_accept = true\n", "[stacks]\nauto_accept = false\n"),
    ("workspace_empty_file", "[jobs]\nworker_concurrency = 6\n", ""),
    ("workspace_without_global_file", None, "[jobs]\nlease_seconds = 120\n"),
    ("workspace_unknown_key", None, "bogus = 1\n"),
    ("workspace_invalid_merged_value", None, "[execution]\nmax_concurrent_runs = 0\n"),
    ("workspace_invalid_toml", None, "not = [valid\n"),
    ("workspace_wrong_section_type", None, "[execution]\nmax_concurrent_runs = true\n"),
    ("global_invalid_no_workspace", "[server]\nport = 0\n", None),
]

# config set inputs: (name, key, raw value text).
SET_CASES: list[tuple[str, str, str]] = [
    ("integer_value", "execution.max_concurrent_runs", "4"),
    ("quoted_json_string_for_integer", "execution.max_concurrent_runs", '"lots"'),
    ("bare_text_is_string", "execution.default_agent", "command"),
    ("json_string", "execution.default_agent", '"command"'),
    ("json_boolean", "llm.auto_classify", "false"),
    (
        "json_object_model_selection",
        "llm.classification",
        '{"provider": "heuristic", "model": "heuristic-v1"}',
    ),
    ("json_list_networks", "server.lan_networks", '["10.0.0.0/8", "192.168.0.0/16"]'),
    ("bad_network", "server.lan_networks", '["nope"]'),
    ("null_default_workspace", "default_workspace", "null"),
    ("string_default_workspace", "default_workspace", "acme"),
    ("unknown_key", "execution.bogus", "1"),
    ("not_nested_under_scalar", "server.host.x", "1"),
    ("port_out_of_range", "server.port", "70000"),
    ("number_for_string", "server.host", "5"),
    ("isolation_not_allowed", "execution.isolation", '"shared"'),
    ("float_timeout_from_int", "llm.providers.ollama.timeout_seconds", "5"),
    (
        "create_openai_provider",
        "llm.providers.lab",
        '{"type": "openai_compatible", "base_url": "http://127.0.0.1:8080/v1"}',
    ),
    ("create_command_agent", "agents.runner", '{"type": "command", "executable": "/bin/echo", "env": {}}'),
]

# config set --workspace inputs: (name, key, raw value text).
WORKSPACE_SET_CASES: list[tuple[str, str, str]] = [
    ("ws_integer", "execution.max_concurrent_runs", "7"),
    ("ws_zero_rejected_on_effective", "execution.max_concurrent_runs", "0"),
    ("ws_boolean", "stacks.auto_accept", "false"),
]

# writer.json inputs: (name, value).
WRITER_CASES: list[tuple[str, dict[str, Any]]] = [
    ("default_global", GlobalConfig().model_dump(mode="json", exclude_none=True)),
    (
        "workspace_override",
        {"schema_version": CONFIG_SCHEMA_VERSION, "execution": {"max_concurrent_runs": 7}},
    ),
    (
        "scalars_and_escapes",
        {
            "s": 'quote " backslash \\ tab \t newline \n unicode é 漢 emoji \U0001f642 ctrl \u0001',
            "f": 1.5,
            "big": 1e20,
            "small": 1e-07,
            "whole": 120.0,
            "b": True,
            "i": -3,
            "empty_list": [],
            "list": ["a", "b"],
            "empty_table": {},
            "nested": {"x": {"y": {"z": 1}}},
            "dotted key": {"needs quote": 1, "bare_key": 2},
        },
    ),
    (
        "provider_and_tracker_tables",
        {
            "llm": {
                "providers": {
                    "lab": {
                        "type": "openai_compatible",
                        "base_url": "http://127.0.0.1:8080/v1",
                        "timeout_seconds": 120.0,
                    },
                    "ollama": {"type": "ollama", "max_retries": 0},
                }
            },
            "trackers": {
                "acme": {
                    "type": "jira",
                    "projects": ["ACME"],
                    "cancelled_resolutions": ["Won't Do", "Duplicate"],
                    "pr_fields": [],
                }
            },
        },
    ),
]


def _store(home: Path) -> ConfigStore:
    paths = AppPaths.resolve(home)
    paths.ensure()
    return ConfigStore(paths)


def _normalise(errors: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [{"type": e["type"], "loc": list(e["loc"]), "msg": e["msg"]} for e in errors]


def _config_error(exc: ConfigError) -> dict[str, Any]:
    errors = (exc.details or {}).get("errors")
    if errors is None:
        return {"outcome": "error", "kind": "invalid_toml", "errors": []}
    return {"outcome": "error", "kind": "validation", "errors": _normalise(errors)}


def _with_home(files: dict[str, str], action: Callable[[ConfigStore], dict[str, Any]]) -> dict[str, Any]:
    """Write ``files`` under a fresh config directory, run ``action`` and return its result."""
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(Path(tmp))
        for rel, text in files.items():
            target = store.paths.config_dir / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(text.encode("utf-8"))
        try:
            return action(store)
        except ConfigError as exc:
            return _config_error(exc)
        except Exception as exc:  # recorded as Python's observed behaviour, not hidden
            return {"outcome": "exception", "type": type(exc).__name__}


def _files_after(store: ConfigStore) -> dict[str, str]:
    found: dict[str, str] = {}
    if store.paths.config_file.exists():
        found["config.toml"] = store.paths.config_file.read_bytes().decode("utf-8")
    if store.paths.workspace_config_dir.exists():
        for file in sorted(store.paths.workspace_config_dir.glob("*.toml")):
            found[f"workspaces/{file.name}"] = file.read_bytes().decode("utf-8")
    return found


def _get(doc: dict[str, Any], dotted: str) -> Any:
    for part in dotted.split("."):
        doc = doc[part]
    return doc


def _put(doc: dict[str, Any], dotted: str, value: Any) -> None:
    parts = dotted.split(".")
    cursor = doc
    for part in parts[:-1]:
        cursor = cursor.setdefault(part, {})
    cursor[parts[-1]] = value


def _load_action(store: ConfigStore) -> dict[str, Any]:
    problems = store.validate_files()
    try:
        cfg = store.load_global()
    except ConfigError as exc:
        return {**_config_error(exc), "problems": len(problems)}
    return {"outcome": "ok", "effective": cfg.model_dump(mode="json"), "problems": len(problems)}


def _apply_cli_set(base: dict[str, Any], key: str, raw: str) -> tuple[Any, dict[str, Any] | None]:
    """Mirror of ``config_set`` in cli/commands/core.py: JSON when it parses, else the raw text."""
    try:
        parsed: Any = json.loads(raw)
    except json.JSONDecodeError:
        parsed = raw
    target = copy.deepcopy(base)
    cursor = target
    parts = key.split(".")
    for part in parts[:-1]:
        cursor = cursor.setdefault(part, {})
        if not isinstance(cursor, dict):
            return parsed, None
    cursor[parts[-1]] = parsed
    return parsed, target


def _set_global(key: str, raw: str) -> dict[str, Any]:
    base = GlobalConfig().model_dump(mode="json")
    parsed, target = _apply_cli_set(base, key, raw)
    if target is None:
        return {"exit": 2, "outcome": "not_nested", "parsed": parsed}
    try:
        cfg = GlobalConfig.model_validate(target)
    except ValidationError as exc:
        return {"exit": 7, "outcome": "invalid", "parsed": parsed, "errors": _normalise(exc.errors())}
    text = tomli_w.dumps(cfg.model_dump(mode="json", exclude_none=True))
    return {
        "exit": 0,
        "outcome": "written",
        "parsed": parsed,
        "value_at_key": _get(cfg.model_dump(mode="json"), key),
        "toml": text,
    }


def _set_workspace(key: str, raw: str) -> dict[str, Any]:
    def action(store: ConfigStore) -> dict[str, Any]:
        base = store.load_workspace_overrides(SLUG).model_dump(mode="json", exclude_none=True)
        parsed, target = _apply_cli_set(base, key, raw)
        if target is None:
            return {"exit": 2, "outcome": "not_nested", "parsed": parsed}
        try:
            overrides = WorkspaceOverrides.model_validate(target)
        except ValidationError as exc:
            return {"exit": 7, "outcome": "invalid", "parsed": parsed, "errors": _normalise(exc.errors())}
        store.save_workspace_overrides(SLUG, overrides)
        try:
            effective = store.effective(SLUG)
        except ConfigError as exc:
            return {"exit": 7, **_config_error(exc), "parsed": parsed}
        return {
            "exit": 0,
            "outcome": "written",
            "parsed": parsed,
            "value_at_key": _get(effective.model_dump(mode="json"), key),
            "workspace_toml": tomli_w.dumps(overrides.model_dump(mode="json", exclude_none=True)),
        }

    return _with_home({}, action)


def _write(name: str, payload: Any) -> None:
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT_DIR / name).write_bytes(text.encode("utf-8"))


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    base_doc = GlobalConfig().model_dump(mode="json", exclude_none=True)
    full_defaults = GlobalConfig().model_dump(mode="json")

    _write(
        "defaults.json",
        {
            "model_default": full_defaults,
            "saved_toml": tomli_w.dumps(base_doc),
            "workspace_default_saved_toml": tomli_w.dumps(
                WorkspaceOverrides().model_dump(mode="json", exclude_none=True)
            ),
        },
    )

    keys: list[dict[str, Any]] = []
    for key, ctype, contract_default, allowed, valid, invalid in KEY_ROWS:
        python_default = _get(full_defaults, key)
        cases = []
        for label, value in (("valid", valid), ("invalid", invalid)):
            doc = copy.deepcopy(base_doc)
            _put(doc, key, value)
            files = {"config.toml": tomli_w.dumps(doc)}
            result = _with_home(files, lambda s, k=key: _key_result(s, k))
            cases.append({"label": label, "input": value, "document_toml": files["config.toml"], **result})
        keys.append(
            {
                "key": key,
                "contract_type": ctype,
                "contract_default": contract_default,
                "contract_allowed": allowed,
                "python_default": python_default,
                "default_matches_contract": python_default == contract_default,
                "cases": cases,
            }
        )
    for table, valid, invalid in TABLE_ROWS:
        cases = []
        for label, value in (("valid", valid), ("invalid", invalid)):
            doc = copy.deepcopy(base_doc)
            _put(doc, table, value)
            files = {"config.toml": tomli_w.dumps(doc)}
            result = _with_home(files, _load_action)
            cases.append({"label": label, "input": value, "document_toml": files["config.toml"], **result})
        keys.append({"key": table, "wildcard": True, "cases": cases})
    _write("keys.json", keys)

    loads = []
    for name, text in LOAD_CASES:
        loads.append({"name": name, "config_toml": text, **_with_home({"config.toml": text}, _load_action)})
    _write("load.json", loads)

    migrates = []
    for name, files in MIGRATE_CASES:

        def migrate_action(store: ConfigStore) -> dict[str, Any]:
            rewritten = [p.relative_to(store.paths.config_dir).as_posix() for p in store.migrate_files()]
            return {"outcome": "ok", "rewritten": rewritten, "files_after": _files_after(store)}

        migrates.append({"name": name, "files_before": files, **_with_home(files, migrate_action)})
    _write("migrate.json", migrates)

    merges = []
    for name, global_text, workspace_text in MERGE_CASES:
        files: dict[str, str] = {}
        if global_text is not None:
            files["config.toml"] = global_text
        if workspace_text is not None:
            files[f"workspaces/{SLUG}.toml"] = workspace_text

        def merge_action(store: ConfigStore) -> dict[str, Any]:
            problems = store.validate_files()
            try:
                effective = store.effective(SLUG)
            except ConfigError as exc:
                return {**_config_error(exc), "problems": len(problems)}
            return {
                "outcome": "ok",
                "effective": effective.model_dump(mode="json"),
                "problems": len(problems),
            }

        merges.append({"name": name, "files": files, "workspace": SLUG, **_with_home(files, merge_action)})
    _write("merge.json", merges)

    sets = []
    for name, key, raw in SET_CASES:
        sets.append({"name": name, "scope": "global", "key": key, "raw": raw, **_set_global(key, raw)})
    for name, key, raw in WORKSPACE_SET_CASES:
        sets.append({"name": name, "scope": "workspace", "key": key, "raw": raw, **_set_workspace(key, raw)})
    _write("set.json", sets)

    writer = []
    for name, value in WRITER_CASES:
        writer.append({"name": name, "value": value, "toml": tomli_w.dumps(value)})
    _write("writer.json", writer)


def _key_result(store: ConfigStore, key: str) -> dict[str, Any]:
    problems = store.validate_files()
    try:
        cfg = store.load_global()
    except ConfigError as exc:
        return {**_config_error(exc), "problems": len(problems)}
    return {
        "outcome": "ok",
        "value_at_key": _get(cfg.model_dump(mode="json"), key),
        "problems": len(problems),
    }


if __name__ == "__main__":
    main()
