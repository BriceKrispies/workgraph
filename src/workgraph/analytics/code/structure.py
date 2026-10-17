"""Language-aware change analysis with Tree-sitter.

Given the old and new content of a file and the changed line ranges from
``git diff -U0``, determine which functions, methods and types were added,
modified or deleted. Files in unsupported languages, or files that fail to parse,
fall back to line-level analysis (reported as ``fallback`` with a reason).
"""

from __future__ import annotations

import re
import threading
from collections.abc import Callable
from dataclasses import dataclass, field
from functools import cache
from typing import Any

ANALYZER_VERSION = "ts-1"

FUNCTION = "function"
TYPE = "type"

# node type -> symbol kind, per grammar
SYMBOL_NODES: dict[str, dict[str, str]] = {
    "python": {"function_definition": FUNCTION, "class_definition": TYPE},
    "javascript": {
        "function_declaration": FUNCTION,
        "generator_function_declaration": FUNCTION,
        "method_definition": FUNCTION,
        "class_declaration": TYPE,
    },
    "typescript": {
        "function_declaration": FUNCTION,
        "generator_function_declaration": FUNCTION,
        "method_definition": FUNCTION,
        "method_signature": FUNCTION,
        "class_declaration": TYPE,
        "abstract_class_declaration": TYPE,
        "interface_declaration": TYPE,
        "type_alias_declaration": TYPE,
        "enum_declaration": TYPE,
    },
    "go": {"function_declaration": FUNCTION, "method_declaration": FUNCTION, "type_spec": TYPE},
    "rust": {
        "function_item": FUNCTION,
        "struct_item": TYPE,
        "enum_item": TYPE,
        "trait_item": TYPE,
        "impl_item": TYPE,
        "union_item": TYPE,
    },
    "c_sharp": {
        "method_declaration": FUNCTION,
        "constructor_declaration": FUNCTION,
        "local_function_statement": FUNCTION,
        "class_declaration": TYPE,
        "struct_declaration": TYPE,
        "interface_declaration": TYPE,
        "record_declaration": TYPE,
        "enum_declaration": TYPE,
    },
}
SYMBOL_NODES["tsx"] = SYMBOL_NODES["typescript"]


def _loaders() -> dict[str, Callable[[], Any]]:
    def python() -> Any:
        import tree_sitter_python

        return tree_sitter_python.language()

    def javascript() -> Any:
        import tree_sitter_javascript

        return tree_sitter_javascript.language()

    def typescript() -> Any:
        import tree_sitter_typescript

        return tree_sitter_typescript.language_typescript()

    def tsx() -> Any:
        import tree_sitter_typescript

        return tree_sitter_typescript.language_tsx()

    def go() -> Any:
        import tree_sitter_go

        return tree_sitter_go.language()

    def rust() -> Any:
        import tree_sitter_rust

        return tree_sitter_rust.language()

    def c_sharp() -> Any:
        import tree_sitter_c_sharp

        return tree_sitter_c_sharp.language()

    return {
        "python": python,
        "javascript": javascript,
        "typescript": typescript,
        "tsx": tsx,
        "go": go,
        "rust": rust,
        "c_sharp": c_sharp,
    }


# The tree-sitter bindings are not thread-safe (grammar loading, parsing and node traversal all
# crashed with access violations when job workers analysed commits concurrently), so every use of
# tree-sitter in this module runs under this lock.
_TREE_SITTER_LOCK = threading.RLock()


@cache
def _parser(language: str) -> Any:
    from tree_sitter import Language, Parser

    loader = _loaders().get(language)
    if loader is None:
        return None
    return Parser(Language(loader()))  # ty: ignore[deprecated]


def available_languages() -> list[str]:
    names = []
    for name in _loaders():
        try:
            with _TREE_SITTER_LOCK:
                parser = _parser(name)
            if parser is not None:
                names.append(name)
        except Exception:  # noqa: S112 - a broken grammar wheel only disables that language
            continue
    return names


@dataclass(frozen=True)
class Symbol:
    kind: str
    name: str
    start_line: int  # 1-based, inclusive
    end_line: int

    def contains(self, line: int) -> bool:
        return self.start_line <= line <= self.end_line

    @property
    def size(self) -> int:
        return self.end_line - self.start_line


@dataclass
class SymbolChange:
    kind: str
    name: str
    change: str  # added | modified | deleted
    start_line: int | None
    end_line: int | None
    lines_changed: int


@dataclass
class FileStructure:
    path: str
    language: str | None
    mode: str  # structural | fallback
    reason: str | None = None
    changes: list[SymbolChange] = field(default_factory=list)
    unattributed_lines: int = 0

    @property
    def functions(self) -> int:
        return len({c.name for c in self.changes if c.kind == FUNCTION})

    @property
    def types(self) -> int:
        return len({c.name for c in self.changes if c.kind == TYPE})


def _name_of(node: Any, language: str) -> str:
    named = node.child_by_field_name("name")
    if named is not None and named.text is not None:
        return named.text.decode("utf-8", errors="replace")
    if language == "rust" and node.type == "impl_item":
        target = node.child_by_field_name("type")
        trait = node.child_by_field_name("trait")
        if target is not None and target.text is not None:
            label = target.text.decode("utf-8", errors="replace")
            if trait is not None and trait.text is not None:
                return f"impl {trait.text.decode('utf-8', errors='replace')} for {label}"
            return f"impl {label}"
    return f"<anonymous {node.type}@{node.start_point.row + 1}>"


def extract_symbols(source: bytes, language: str) -> list[Symbol] | None:
    with _TREE_SITTER_LOCK:
        return _extract_symbols(source, language)


def _extract_symbols(source: bytes, language: str) -> list[Symbol] | None:
    parser = _parser(language)
    if parser is None:
        return None
    tree = parser.parse(source)
    kinds = SYMBOL_NODES[language]
    symbols: list[Symbol] = []

    def walk(node: Any, scope: tuple[str, ...]) -> None:
        kind = kinds.get(node.type)
        next_scope = scope
        if kind is not None:
            name = ".".join((*scope, _name_of(node, language)))
            symbols.append(Symbol(kind, name, node.start_point.row + 1, node.end_point.row + 1))
            next_scope = (*scope, _name_of(node, language))
        for child in node.children:
            walk(child, next_scope)

    walk(tree.root_node, ())
    if tree.root_node.has_error and not symbols:
        return None
    return symbols


_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def parse_hunks(diff_text: str) -> tuple[list[int], list[int]]:
    """Old-side deleted line numbers and new-side added line numbers from a ``-U0`` diff."""
    deleted: list[int] = []
    added: list[int] = []
    for line in diff_text.splitlines():
        match = _HUNK.match(line)
        if not match:
            continue
        old_start, old_len = int(match.group(1)), int(match.group(2) or "1")
        new_start, new_len = int(match.group(3)), int(match.group(4) or "1")
        deleted.extend(range(old_start, old_start + old_len))
        added.extend(range(new_start, new_start + new_len))
    return deleted, added


def _innermost(symbols: list[Symbol], line: int) -> Symbol | None:
    best: Symbol | None = None
    for symbol in symbols:
        if symbol.contains(line) and (best is None or symbol.size <= best.size):
            best = symbol
    return best


def analyze_file(
    path: str,
    language: str | None,
    old_source: bytes | None,
    new_source: bytes | None,
    deleted_lines: list[int],
    added_lines: list[int],
) -> FileStructure:
    if language is None or language not in SYMBOL_NODES:
        return FileStructure(path, language, "fallback", reason="unsupported language")
    try:
        old_symbols = extract_symbols(old_source, language) if old_source is not None else []
        new_symbols = extract_symbols(new_source, language) if new_source is not None else []
    except Exception as exc:  # grammar crash -> line-level fallback
        return FileStructure(path, language, "fallback", reason=f"parse error: {exc}")
    if old_symbols is None or new_symbols is None:
        return FileStructure(path, language, "fallback", reason="unparsable source")
    old_names = {s.name for s in old_symbols}
    new_names = {s.name for s in new_symbols}
    touched: dict[tuple[str, str], SymbolChange] = {}
    unattributed = 0
    for line in added_lines:
        symbol = _innermost(new_symbols, line)
        if symbol is None:
            unattributed += 1
            continue
        change = "added" if symbol.name not in old_names else "modified"
        key = (symbol.kind, symbol.name)
        entry = touched.setdefault(
            key, SymbolChange(symbol.kind, symbol.name, change, symbol.start_line, symbol.end_line, 0)
        )
        entry.lines_changed += 1
    for line in deleted_lines:
        symbol = _innermost(old_symbols, line)
        if symbol is None:
            unattributed += 1
            continue
        change = "deleted" if symbol.name not in new_names else "modified"
        key = (symbol.kind, symbol.name)
        if key in touched:
            touched[key].lines_changed += 1
        else:
            touched[key] = SymbolChange(
                symbol.kind, symbol.name, change, symbol.start_line, symbol.end_line, 1
            )
    return FileStructure(
        path, language, "structural", changes=list(touched.values()), unattributed_lines=unattributed
    )
