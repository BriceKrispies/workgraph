"""GV-14 golden vectors: Pydantic validation of the request models and MCP tool arguments.

Writes ``golden/pydantic/`` (run ``uv run python tools/golden/gen_pydantic.py``):

* ``request/<Model>.json`` for every ``InputSchema`` subclass in ``workgraph.core.models.schemas``;
* ``mcp/<tool>.json`` for every MCP tool registered by ``workgraph.mcp.server.build_server``, validated
  through the argument model the MCP SDK builds from the tool's signature.

Each file holds a ``base`` payload that validates and a list of ``cases``. A case is one payload plus the
result Python produces: ``{"ok": <model_dump(mode="json")>}`` or ``{"errors": [...]}``, where each error
keeps Pydantic's ``type``, ``loc``, ``msg``, ``input`` and ``ctx`` (``url`` is dropped). Probes per field:
missing, one wrong value per common JSON type, the field's own bounds just outside their limits, an
invalid string or list of strings for validators such as slug and tag patterns, and for integer fields
integral floats around the i64 range. Each model also gets an
unknown key and a non-object body.

Output is deterministic: sorted keys, ``\\n`` line endings, UTF-8.
"""

from __future__ import annotations

import enum
import json
import sys
import types
from datetime import datetime
from pathlib import Path
from typing import Annotated, Any, Literal, Union, get_args, get_origin

# This file is named ``pydantic.py``, so the directory holding it must not be on sys.path: otherwise
# ``import pydantic`` would import this script. Running it as a script puts that directory first.
_HERE = Path(__file__).resolve().parent
sys.path = [p for p in sys.path if Path(p or ".").resolve() != _HERE]

from pydantic import BaseModel, ValidationError  # noqa: E402
from pydantic_core import PydanticUndefined  # noqa: E402

from workgraph.core.models import schemas  # noqa: E402
from workgraph.mcp.server import build_server  # noqa: E402

ROOT = _HERE.parents[1]
OUT = ROOT / "golden" / "pydantic"

WRONG_TYPE_PROBES: list[tuple[str, Any]] = [
    ("none", None),
    ("int", 12),
    ("float", 1.5),
    ("bool", True),
    ("str", "x"),
    ("list", ["a"]),
    ("dict", {"k": 1}),
]
# Floats probed on integer fields: far outside the i64 range (Pydantic: int_parsing_size), exactly 2**63
# (also int_parsing_size, the range is open), and a large integral float inside it (accepted as an int).
INT_FLOAT_PROBES: list[tuple[str, float]] = [
    ("huge", 1.5e300),
    ("edge", 9.223372036854776e18),
    ("large", 9.2e18),
]
INVALID_TEXT = "Not valid!"
BASE_TEXT = "example"
BASE_TIME = "2026-10-25T12:00:00Z"
MAX_PROBE_LENGTH = 600


def _plain(value: Any) -> Any:
    """Turn a Python value from a Pydantic error into plain JSON data."""
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, enum.Enum):
        return _plain(value.value)
    if isinstance(value, datetime):
        return value.isoformat()
    if value is None or isinstance(value, (str, bool, int, float)):
        return value
    return str(value)


def _errors(exc: ValidationError) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for err in exc.errors(include_url=False):
        item: dict[str, Any] = {
            "type": err["type"],
            "loc": _plain(list(err["loc"])),
            "msg": err["msg"],
            "input": _plain(err["input"]),
        }
        if "ctx" in err:
            item["ctx"] = _plain(err["ctx"])
        out.append(item)
    return out


def _run(model: type[BaseModel], payload: Any) -> dict[str, Any]:
    try:
        obj = model.model_validate(payload)
    except ValidationError as exc:
        return {"errors": _errors(exc)}
    return {"ok": _plain(obj.model_dump(mode="json"))}


def _unwrap(annotation: Any) -> Any:
    while get_origin(annotation) is Annotated:
        annotation = get_args(annotation)[0]
    origin = get_origin(annotation)
    if origin in (Union, types.UnionType):
        non_none = [a for a in get_args(annotation) if a is not type(None)]
        return _unwrap(non_none[0]) if non_none else None
    if origin is Literal:
        return ("literal", get_args(annotation)[0])
    return annotation


def _sample(annotation: Any) -> Any:
    """A valid example value for a field annotation."""
    resolved = _unwrap(annotation)
    if isinstance(resolved, tuple) and resolved[0] == "literal":
        return resolved[1]
    origin = get_origin(resolved)
    if origin is list:
        return []
    if origin is dict:
        return {}
    if resolved is Any or resolved is None:
        return BASE_TEXT
    if isinstance(resolved, type):
        if issubclass(resolved, enum.Enum):
            return next(iter(resolved)).value
        if issubclass(resolved, BaseModel):
            return {
                name: _sample(field.annotation)
                for name, field in resolved.model_fields.items()
                if field.default is PydanticUndefined and field.default_factory is None
            }
        if resolved is bool:
            return False
        if resolved is int:
            return 1
        if resolved is float:
            return 1.0
        if resolved is str:
            return BASE_TEXT
        if resolved is datetime:
            return BASE_TIME
        if resolved is dict:
            return {}
        if resolved is list:
            return []
    return BASE_TEXT


def _field_sample(field: Any) -> Any:
    """A valid example for a field, honouring a non-empty list minimum (``min_length >= 1``)."""
    resolved = _unwrap(field.annotation)
    needs_item = any(getattr(meta, "min_length", 0) for meta in field.metadata)
    if get_origin(resolved) is list and needs_item:
        return [_sample(get_args(resolved)[0])]
    return _sample(field.annotation)


def _base(fields: dict[str, Any]) -> dict[str, Any]:
    """Payload with every required field set to a valid example (optional fields take their defaults)."""
    return {
        field.alias or name: _field_sample(field)
        for name, field in fields.items()
        if field.default is PydanticUndefined and field.default_factory is None
    }


def _bounded_probes(name: str, field: Any, base_value: Any) -> list[tuple[str, Any]]:
    """Out-of-bound values taken from the field's own constraints, one step outside each limit."""
    probes: list[tuple[str, Any]] = []
    for meta in field.metadata:
        if hasattr(meta, "min_length") and getattr(meta, "min_length", None) is not None:
            size = meta.min_length - 1
            if isinstance(base_value, str):
                probes.append((f"min_length:{name}", "x" * size))
            elif isinstance(base_value, list):
                probes.append((f"min_length:{name}", ["a"] * size))
        if getattr(meta, "max_length", None) is not None:
            size = meta.max_length + 1
            if size <= MAX_PROBE_LENGTH:
                if isinstance(base_value, str):
                    probes.append((f"max_length:{name}", "x" * size))
                elif isinstance(base_value, list):
                    probes.append((f"max_length:{name}", ["a"] * size))
        if getattr(meta, "ge", None) is not None:
            probes.append((f"ge:{name}", meta.ge - 1))
        if getattr(meta, "gt", None) is not None:
            probes.append((f"gt:{name}", meta.gt))
        if getattr(meta, "le", None) is not None:
            probes.append((f"le:{name}", meta.le + 1))
        if getattr(meta, "lt", None) is not None:
            probes.append((f"lt:{name}", meta.lt))
    return probes


def _cases(model: type[BaseModel], fields: dict[str, Any]) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    base = _base(fields)
    cases: list[dict[str, Any]] = []

    def add(case_id: str, payload: Any) -> None:
        cases.append({"id": case_id, "input": payload, "output": _run(model, payload)})

    add("base", base)
    for name, field in fields.items():
        key = field.alias or name
        if key in base:
            add(f"missing:{name}", {k: v for k, v in base.items() if k != key})
        for label, value in WRONG_TYPE_PROBES:
            add(f"wrong:{name}:{label}", {**base, key: value})
        if _unwrap(field.annotation) is int:
            for label, value in INT_FLOAT_PROBES:
                add(f"float:{name}:{label}", {**base, key: value})
        sample = base.get(key, _field_sample(field))
        if isinstance(sample, str):
            add(f"invalid:{name}", {**base, key: INVALID_TEXT})
        if isinstance(sample, list):
            add(f"invalid:{name}", {**base, key: [INVALID_TEXT]})
        for case_id, value in _bounded_probes(name, field, sample):
            add(case_id, {**base, key: value})
    add("extra:unknown_key", {**base, "unknown_key": 1})
    add("body:not_object", [])
    return base, cases


def _write(path: Path, document: dict[str, Any]) -> int:
    text = json.dumps(document, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    data = text.encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return len(data)


def _request_models() -> list[type[BaseModel]]:
    found: dict[str, type[BaseModel]] = {}
    pending = list(schemas.InputSchema.__subclasses__())
    while pending:
        cls = pending.pop()
        if cls.__name__ in found:
            continue
        found[cls.__name__] = cls
        pending.extend(cls.__subclasses__())
    return [found[name] for name in sorted(found)]


def main() -> None:
    if OUT.exists():
        for stale in OUT.rglob("*.json"):
            stale.unlink()
    total = 0
    summary: list[str] = []

    for model in _request_models():
        base, cases = _cases(model, model.model_fields)
        assert "ok" in cases[0]["output"], f"base payload of {model.__name__} does not validate"
        document = {"base": base, "cases": cases, "kind": "request", "model": model.__name__}
        size = _write(OUT / "request" / f"{model.__name__}.json", document)
        assert size < 2_000_000, model.__name__
        total += len(cases)
        summary.append(f"request {model.__name__}: {len(cases)} cases, {size} bytes")

    server = build_server(None)  # the app is only touched inside tool bodies, never at registration
    for tool in sorted(server._tool_manager.list_tools(), key=lambda t: t.name):
        arg_model = tool.fn_metadata.arg_model
        base, cases = _cases(arg_model, arg_model.model_fields)
        assert "ok" in cases[0]["output"], f"base arguments of {tool.name} do not validate"
        document = {"base": base, "cases": cases, "kind": "mcp", "tool": tool.name}
        size = _write(OUT / "mcp" / f"{tool.name}.json", document)
        assert size < 2_000_000, tool.name
        total += len(cases)
        summary.append(f"mcp {tool.name}: {len(cases)} cases, {size} bytes")

    print("\n".join(summary))
    print(f"total cases: {total}")


if __name__ == "__main__":
    main()
