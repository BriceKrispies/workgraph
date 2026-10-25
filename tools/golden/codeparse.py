"""Golden vectors for language-aware change analysis (GV-16).

Writes golden/codeparse/*.json from the Python analyzer in analytics/code/structure.py:

* extract.json: symbol extraction for one source per language (the sources of
  tests/unit/test_foundations.py plus a 300-class Python file).
* <language>.json: before and after sources with added, modified and deleted functions and types,
  the -U0 hunks, the parsed line numbers and the expected FileStructure.
* fallback.json: unsupported languages, added and deleted files, unattributed lines, unparsable
  sources, and the hunk example of test_analyze_file_changes_and_fallback.
* grammars.json: the analyzer version and the installed tree-sitter and grammar versions.

Run from the repository root: uv run python tools/golden/codeparse.py
Output is sorted-key JSON with UTF-8 and LF line endings, so repeated runs give identical bytes.
"""

from __future__ import annotations

import difflib
import json
import textwrap
from dataclasses import asdict
from importlib import metadata
from pathlib import Path
from typing import Any

from workgraph.analytics.code.structure import (
    ANALYZER_VERSION,
    SYMBOL_NODES,
    analyze_file,
    available_languages,
    extract_symbols,
    parse_hunks,
)

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "codeparse"

GRAMMAR_PACKAGES = [
    "tree-sitter",
    "tree-sitter-c-sharp",
    "tree-sitter-go",
    "tree-sitter-javascript",
    "tree-sitter-python",
    "tree-sitter-rust",
    "tree-sitter-typescript",
]

CHANGES_COMPARE = "changes are sorted by (kind, name, change); compare as lists after that sort"


def src(text: str) -> str:
    return textwrap.dedent(text).lstrip("\n")


# Sources of tests/unit/test_foundations.py (SOURCES) and its 300-class thread-safety input.
EXTRACT_SOURCES: dict[str, str] = {
    "python": "class A:\n    def m(self):\n        return 1\n",
    "typescript": "export class Svc {\n  run(): number {\n    return 1;\n  }\n}\n",
    "tsx": "function App() {\n  return <div/>;\n}\n",
    "javascript": "function go() {\n  return 1;\n}\n",
    "go": "package main\n\nfunc Run() int {\n\treturn 1\n}\n",
    "rust": "struct S;\nimpl S {\n    fn run(&self) -> i32 {\n        1\n    }\n}\n",
    "c_sharp": "class Svc {\n  int Run() {\n    return 1;\n  }\n}\n",
}
MANY_CLASSES = "\n".join(f"class C{i}:\n    def m(self):\n        return {i}" for i in range(300))

# language -> [(case id, old source, new source)]; each pair adds, modifies and deletes
# functions and types.
CHANGE_CASES: dict[str, list[tuple[str, str, str]]] = {
    "python": [
        (
            "store-cache",
            src(
                """
                import os

                class Store:
                    def load(self, path):
                        return open(path).read()

                    def save(self, path, data):
                        with open(path, "w") as fh:
                            fh.write(data)


                def helper(x):
                    return x + 1


                def legacy(x):
                    return x * 2
                """
            ),
            src(
                """
                import os

                class Store:
                    def load(self, path):
                        return open(path, encoding="utf-8").read()

                    def save(self, path, data):
                        with open(path, "w") as fh:
                            fh.write(data)

                    def flush(self):
                        return None


                class Cache:
                    size = 10


                def helper(x):
                    return x + 1
                """
            ),
        ),
        (
            "nested-scopes",
            src(
                """
                class Outer:
                    class Inner:
                        def run(self):
                            return 1

                    def keep(self):
                        return 2


                def gone():
                    return 3
                """
            ),
            src(
                """
                class Outer:
                    class Inner:
                        def run(self):
                            return 10

                        def stop(self):
                            return 0

                    def keep(self):
                        return 2


                class Fresh:
                    pass
                """
            ),
        ),
    ],
    "javascript": [
        (
            "parser-lexer",
            src(
                """
                function parse(text) {
                  return text.trim();
                }

                class Lexer {
                  next() {
                    return 1;
                  }
                }

                function drop() {
                  return 0;
                }
                """
            ),
            src(
                """
                function parse(text) {
                  return text.trim().toLowerCase();
                }

                class Lexer {
                  next() {
                    return 1;
                  }

                  peek() {
                    return 2;
                  }
                }

                function* ticks() {
                  yield 1;
                }

                class Token {}
                """
            ),
        ),
        (
            "rename-function",
            src(
                """
                function oldName(a) {
                  return a;
                }
                """
            ),
            src(
                """
                function newName(a) {
                  return a + 1;
                }
                """
            ),
        ),
    ],
    "typescript": [
        (
            "users",
            src(
                """
                export interface User {
                  id: number;
                  name: string;
                }

                export class UserService {
                  find(id: number): User | undefined {
                    return undefined;
                  }
                }

                type Id = number;

                export function make(): UserService {
                  return new UserService();
                }
                """
            ),
            src(
                """
                export interface User {
                  id: number;
                  name: string;
                  email: string;
                }

                export class UserService {
                  find(id: number): User | undefined {
                    return undefined;
                  }

                  remove(id: number): boolean {
                    return false;
                  }
                }

                export enum Role {
                  Admin,
                  Guest,
                }
                """
            ),
        ),
        (
            "abstract-base",
            src(
                """
                export abstract class Base {
                  abstract run(): void;
                }

                export function helper(): number {
                  return 1;
                }
                """
            ),
            src(
                """
                export abstract class Base {
                  abstract run(): void;
                  abstract stop(): void;
                }
                """
            ),
        ),
    ],
    "tsx": [
        (
            "app-panel",
            src(
                """
                import React from "react";

                export function App(props: { name: string }) {
                  return <div>{props.name}</div>;
                }

                export function Old() {
                  return <span />;
                }
                """
            ),
            src(
                """
                import React from "react";

                export function App(props: { name: string }) {
                  return <section>{props.name}</section>;
                }

                export class Panel extends React.Component<{}> {
                  render() {
                    return <div />;
                  }
                }
                """
            ),
        ),
    ],
    "go": [
        (
            "store",
            src(
                """
                package store

                type Store struct {
                    path string
                }

                func (s *Store) Load() string {
                    return s.path
                }

                func Legacy() int {
                    return 1
                }
                """
            ),
            src(
                """
                package store

                type Store struct {
                    path string
                    mode int
                }

                func (s *Store) Load() string {
                    return s.path + "!"
                }

                type Cache interface {
                    Get(key string) string
                }
                """
            ),
        ),
    ],
    "rust": [
        (
            "counter",
            src(
                """
                struct Counter {
                    n: u32,
                }

                impl Counter {
                    fn bump(&mut self) {
                        self.n += 1;
                    }
                }

                fn reset() -> u32 {
                    0
                }
                """
            ),
            src(
                """
                struct Counter {
                    n: u32,
                    step: u32,
                }

                impl Counter {
                    fn bump(&mut self) {
                        self.n += self.step;
                    }

                    fn peek(&self) -> u32 {
                        self.n
                    }
                }

                enum Mode {
                    Fast,
                    Slow,
                }
                """
            ),
        ),
    ],
    "c_sharp": [
        (
            "shop",
            src(
                """
                namespace Shop
                {
                    public class Cart
                    {
                        public int Total() { return 0; }
                    }

                    public static class Legacy
                    {
                        public static int Old() { return 1; }
                    }
                }
                """
            ),
            src(
                """
                namespace Shop
                {
                    public class Cart
                    {
                        public int Total() { return 42; }

                        public void Add(int price) { }
                    }

                    public record Receipt(int Amount);

                    public interface IPricer
                    {
                    }
                }
                """
            ),
        ),
    ],
}

# (case id, language or None, path, old, new, diff override or None)
FALLBACK_CASES: list[tuple[str, str | None, str, str | None, str | None, str | None]] = [
    ("unsupported-no-language", None, "notes.txt", "alpha\n", "beta\n", None),
    ("unsupported-markdown", "markdown", "README.md", "# A\n", "# B\n", None),
    (
        "added-file-python",
        "python",
        "src/new.py",
        None,
        src(
            """
            def first():
                return 1


            class Second:
                pass
            """
        ),
        None,
    ),
    (
        "deleted-file-go",
        "go",
        "cmd/old.go",
        src(
            """
            package main

            func Gone() int {
                return 1
            }
            """
        ),
        None,
        None,
    ),
    (
        "comment-only-python",
        "python",
        "tool.py",
        "x = 1\n\ndef f():\n    return 1\n",
        "# note\nx = 1\n\ndef f():\n    return 1\n",
        None,
    ),
    (
        "unparsable-old-python",
        "python",
        "broken.py",
        "]]] ((( }}}\n",
        "def ok():\n    return 1\n",
        None,
    ),
    (
        "test-foundations-hunks",
        "python",
        "m.py",
        "def a():\n    return 1\n\ndef b():\n    return 2\n",
        "def a():\n    return 10\n\ndef c():\n    return 3\n",
        "@@ -2 +2 @@\n-    return 1\n+    return 10\n@@ -4,2 +4,2 @@\n",
    ),
]


def _range(start: int, length: int) -> str:
    return str(start) if length == 1 else f"{start},{length}"


def unified_u0(old: str | None, new: str | None) -> str:
    """A git-shaped ``-U0`` diff, computed with difflib so that the input is reproducible."""
    old_lines = [] if old is None else old.splitlines(keepends=True)
    new_lines = [] if new is None else new.splitlines(keepends=True)
    matcher = difflib.SequenceMatcher(None, old_lines, new_lines, autojunk=False)
    out: list[str] = []
    for tag, i1, i2, j1, j2 in matcher.get_opcodes():
        if tag == "equal":
            continue
        old_len, new_len = i2 - i1, j2 - j1
        old_start = i1 + 1 if old_len else i1
        new_start = j1 + 1 if new_len else j1
        out.append(f"@@ -{_range(old_start, old_len)} +{_range(new_start, new_len)} @@")
        out.extend("-" + line.rstrip("\n") for line in old_lines[i1:i2])
        out.extend("+" + line.rstrip("\n") for line in new_lines[j1:j2])
    return "\n".join(out) + "\n" if out else ""


def _symbols(source: bytes, language: str) -> list[dict[str, Any]] | None:
    symbols = extract_symbols(source, language)
    return None if symbols is None else [asdict(s) for s in symbols]


def build_case(
    case_id: str,
    language: str | None,
    path: str,
    old: str | None,
    new: str | None,
    diff: str | None = None,
) -> dict[str, Any]:
    diff_text = unified_u0(old, new) if diff is None else diff
    deleted, added = parse_hunks(diff_text)
    old_bytes = None if old is None else old.encode("utf-8")
    new_bytes = None if new is None else new.encode("utf-8")
    if language in SYMBOL_NODES:
        symbols_old = [] if old_bytes is None else _symbols(old_bytes, language)
        symbols_new = [] if new_bytes is None else _symbols(new_bytes, language)
    else:
        symbols_old = symbols_new = None
    result = analyze_file(path, language, old_bytes, new_bytes, deleted, added)
    changes = sorted(
        (
            {
                "kind": c.kind,
                "name": c.name,
                "change": c.change,
                "start_line": c.start_line,
                "end_line": c.end_line,
                "lines_changed": c.lines_changed,
            }
            for c in result.changes
        ),
        key=lambda c: (c["kind"], c["name"], c["change"]),
    )
    return {
        "id": case_id,
        "language": language,
        "path": path,
        "old": old,
        "new": new,
        "diff": diff_text,
        "deleted_lines": deleted,
        "added_lines": added,
        "symbols_old": symbols_old,
        "symbols_new": symbols_new,
        "expected": {
            "mode": result.mode,
            "reason": result.reason,
            "functions": result.functions,
            "types": result.types,
            "unattributed_lines": result.unattributed_lines,
            "changes": changes,
        },
    }


def dump(name: str, obj: Any) -> None:
    text = json.dumps(obj, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT / name).write_bytes(text.encode("utf-8"))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    extract_cases = []
    for language in sorted(EXTRACT_SOURCES):
        source = EXTRACT_SOURCES[language].encode("utf-8")
        extract_cases.append(
            {
                "id": f"extract-{language}",
                "language": language,
                "source": EXTRACT_SOURCES[language],
                "symbols": _symbols(source, language),
            }
        )
    extract_cases.append(
        {
            "id": "extract-python-many-classes",
            "language": "python",
            "source": MANY_CLASSES,
            "symbols": _symbols(MANY_CLASSES.encode("utf-8"), "python"),
        }
    )
    dump("extract.json", {"cases": extract_cases})

    for language in sorted(CHANGE_CASES):
        cases = [
            build_case(cid, language, f"{cid}.{language}", old, new)
            for cid, old, new in CHANGE_CASES[language]
        ]
        dump(
            f"{language}.json",
            {"analyzer_version": ANALYZER_VERSION, "compare": CHANGES_COMPARE, "cases": cases},
        )

    fallback = [
        build_case(cid, lang, path, old, new, diff) for cid, lang, path, old, new, diff in FALLBACK_CASES
    ]
    dump(
        "fallback.json", {"analyzer_version": ANALYZER_VERSION, "compare": CHANGES_COMPARE, "cases": fallback}
    )

    grammars = {package: metadata.version(package) for package in GRAMMAR_PACKAGES}
    dump(
        "grammars.json",
        {
            "analyzer_version": ANALYZER_VERSION,
            "available_languages": sorted(available_languages()),
            "grammars": grammars,
        },
    )


if __name__ == "__main__":
    main()
