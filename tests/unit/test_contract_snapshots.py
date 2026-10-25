"""Drift test for the contract snapshots under ``contracts/`` (task CF-01).

The snapshots are produced by ``tools/contracts/dump.py`` from the running Python code. This test
rebuilds them and fails when the code and the committed files disagree, so a Python change that alters
a contract has to update the snapshot in the same commit.
"""

from __future__ import annotations

import importlib.util
import json
import re
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[2]
CONTRACTS_DIR = ROOT / "contracts"
DUMP_PATH = ROOT / "tools" / "contracts" / "dump.py"

# Counts measured at 5aecab8 and recorded in docs/rust-rewrite-contracts.md (Counts table).
CLI_LEAF_COMMANDS = 186
CLI_TOP_LEVEL_NAMES = 31
MCP_TOOLS = 66
MCP_STATIC_RESOURCES = 2
MCP_RESOURCE_TEMPLATES = 5
OPENAPI_OPERATIONS = 124
OPENAPI_PATHS = 109
OPENAPI_SCHEMAS = 73
DB_TABLES = 72
DB_INDEXES = 207
DB_AUTOMATIC_INDEXES = 103
DB_TRIGGERS = 2


def _load_dump() -> ModuleType:
    spec = importlib.util.spec_from_file_location("contracts_dump", DUMP_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def dump() -> ModuleType:
    return _load_dump()


@pytest.fixture(scope="module")
def built(dump: ModuleType) -> dict[str, bytes]:
    return dump.build_snapshots()


def _committed(name: str) -> bytes:
    path = CONTRACTS_DIR / name
    assert path.is_file(), f"missing snapshot {path}; run: uv run python tools/contracts/dump.py"
    return path.read_bytes()


def test_snapshot_files_exist(dump: ModuleType) -> None:
    for name in dump.SNAPSHOT_NAMES:
        assert (CONTRACTS_DIR / name).is_file(), name


@pytest.mark.parametrize(
    "name", ["cli.json", "mcp.json", "openapi.json", "config-schema.json", "schema-0010.sql"]
)
def test_committed_snapshot_matches_python_code(built: dict[str, bytes], name: str) -> None:
    assert built[name] == _committed(name), (
        f"{name} drifted from the Python code. If the change is intended, run "
        "`uv run python tools/contracts/dump.py` and commit the snapshot with the code change."
    )


def test_dump_output_is_deterministic(dump: ModuleType, built: dict[str, bytes]) -> None:
    assert dump.build_snapshots() == built


def test_cli_inventory_counts(built: dict[str, bytes]) -> None:
    cli = json.loads(built["cli.json"])
    assert cli["leaf_count"] == CLI_LEAF_COMMANDS
    assert len(cli["top_level_names"]) == CLI_TOP_LEVEL_NAMES
    assert len(cli["commands"]) == CLI_LEAF_COMMANDS


def test_mcp_inventory_counts(built: dict[str, bytes]) -> None:
    mcp = json.loads(built["mcp.json"])
    assert len(mcp["tools"]) == MCP_TOOLS
    assert len(mcp["resources"]) == MCP_STATIC_RESOURCES
    assert len(mcp["resource_templates"]) == MCP_RESOURCE_TEMPLATES


def test_openapi_inventory_counts(built: dict[str, bytes]) -> None:
    openapi = json.loads(built["openapi.json"])
    operations = sum(
        1
        for item in openapi["paths"].values()
        for method in item
        if method in {"get", "put", "post", "delete", "patch", "options", "head", "trace"}
    )
    assert operations == OPENAPI_OPERATIONS
    assert len(openapi["paths"]) == OPENAPI_PATHS
    assert len(openapi["components"]["schemas"]) == OPENAPI_SCHEMAS


def test_database_inventory_counts(built: dict[str, bytes]) -> None:
    text = built["schema-0010.sql"].decode("utf-8")
    tables = len(re.findall(r"^CREATE (?:VIRTUAL )?TABLE ", text, re.MULTILINE))
    indexes = len(re.findall(r"^(?:CREATE (?:UNIQUE )?INDEX |-- automatic index )", text, re.MULTILINE))
    automatic = len(re.findall(r"^-- automatic index ", text, re.MULTILINE))
    triggers = len(re.findall(r"^CREATE TRIGGER ", text, re.MULTILINE))
    assert tables == DB_TABLES
    assert indexes == DB_INDEXES
    assert automatic == DB_AUTOMATIC_INDEXES
    assert triggers == DB_TRIGGERS
