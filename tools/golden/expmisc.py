"""Golden vectors for the experiment helpers: leaf contract, brief rendering, escape detection and replay selection.

Writes golden/expmisc/{contract,briefs,escape,replay}.json. Output is deterministic: sorted keys, LF line endings,
UTF-8, and no paths, clocks or random state beyond the fixed-seed generator below.

Each case lists ``covers``: the Python unit tests (tests/unit/...) whose inputs it reproduces. Generated cases have
an empty ``covers`` list.
"""

from __future__ import annotations

import json
import tempfile
from dataclasses import asdict, replace
from pathlib import Path
from types import SimpleNamespace

from workgraph.core.errors import ValidationFailed
from workgraph.experiments import replay as replay_mod
from workgraph.experiments.briefs import BriefContext, render
from workgraph.experiments.contract import contract_problems
from workgraph.experiments.escape import find_escapes, normalize_path_text
from workgraph.integrations.claude_code.transcript import ToolCall

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "expmisc"


def write(name: str, cases: list[dict]) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text = json.dumps(
        {"vector": "expmisc", "file": name, "cases": cases}, sort_keys=True, indent=2, ensure_ascii=False
    )
    with (OUT / f"{name}.json").open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text + "\n")


class Lcg:
    """Fixed linear congruential generator: the same sequence on every Python version."""

    def __init__(self, seed: int) -> None:
        self.state = seed & 0x7FFFFFFF

    def next(self, n: int) -> int:
        self.state = (self.state * 1103515245 + 12345) & 0x7FFFFFFF
        return (self.state >> 8) % n


# ---------------------------------------------------------------- contract


def _task(tags=(), criteria=None, contract=None):
    criteria = ["the function returns 1"] if criteria is None else list(criteria)
    return SimpleNamespace(
        tags=list(tags),
        acceptance_criteria=criteria,
        metadata={} if contract is None else {"contract": contract},
    )


def _contract(**overrides):
    contract = {
        "files": ["pkg/mod.py", "tests/test_mod.py"],
        "oracle": [["pytest", "-q", "tests/test_mod.py"]],
        "gates": [["ruff", "check", "."]],
        "out_of_scope": ["the CLI"],
        "checks": ["tests/test_mod.py::test_f"],
        "predicted": {"src_lines": 40, "src_files": 1, "packages": ["pkg"]},
    }
    contract.update(overrides)
    return contract


def _contract_case(case_id, covers, tags=(), criteria=None, contract=None):
    task = _task(tags=tags, criteria=criteria, contract=contract)
    missing, not_delegable = contract_problems(task)
    return {
        "id": case_id,
        "covers": covers,
        "input": {
            "tags": list(tags),
            "acceptance_criteria": task.acceptance_criteria,
            "metadata": task.metadata,
        },
        "output": {"missing": list(missing), "not_delegable": list(not_delegable)},
    }


def contract_cases() -> list[dict]:
    cases = [
        _contract_case("complete", ["test_complete_contract_has_no_problems"], contract=_contract()),
    ]
    for tag in ("needs-decision", "kind:report", "kind:register-row", "kind:split-status", "kind:manifest"):
        cases.append(
            _contract_case(
                f"tag-{tag}", ["test_each_not_delegable_tag_is_named"], tags=[tag], contract=_contract()
            )
        )
    cases += [
        _contract_case(
            "unrelated-tag",
            ["test_unrelated_tags_are_delegable"],
            tags=["capability-frontier"],
            contract=_contract(),
        ),
        _contract_case(
            "no-oracle-no-criteria",
            ["test_no_oracle_and_no_criteria_is_not_delegable"],
            criteria=[],
            contract=_contract(oracle=[], checks=[]),
        ),
        _contract_case(
            "criteria-without-oracle",
            ["test_criteria_without_a_contract_oracle_are_still_checkable"],
            contract=_contract(oracle=[]),
        ),
        _contract_case("missing-contract", ["test_missing_contract_names_every_field"]),
        _contract_case(
            "malformed-fields",
            ["test_malformed_fields_are_named"],
            contract=_contract(
                files=[],
                oracle=[["pytest", ""]],
                gates=[[]],
                out_of_scope=[""],
                predicted={"src_lines": 0, "src_files": True, "packages": "pkg"},
            ),
        ),
        _contract_case("gates-empty", ["test_gates_may_be_empty"], contract=_contract(gates=[])),
        _contract_case(
            "packages-empty",
            ["test_packages_may_be_empty"],
            contract=_contract(predicted={"src_lines": 4, "src_files": 1, "packages": []}),
        ),
        _contract_case(
            "checks-count-mismatch",
            ["test_checks_must_match_the_criteria_count"],
            criteria=["one", "two"],
            contract=_contract(checks=["tests/a.py::t"]),
        ),
        _contract_case(
            "blank-check",
            ["test_blank_check_is_malformed"],
            criteria=["one"],
            contract=_contract(checks=["  "]),
        ),
    ]
    # Generated: criteria counts 0..3 against check counts 0..3, with and without a blank check entry.
    for criteria_count in range(4):
        for check_count in range(4):
            for blank in (False, True):
                criteria = [f"criterion {i}" for i in range(criteria_count)]
                checks = [f"tests/t.py::case_{i}" for i in range(check_count)]
                if blank and checks:
                    checks[-1] = "   "
                cases.append(
                    _contract_case(
                        f"generated-criteria{criteria_count}-checks{check_count}-blank{int(blank)}",
                        [],
                        criteria=criteria,
                        contract=_contract(checks=checks),
                    )
                )
    return cases


# ---------------------------------------------------------------- briefs

BRIEF_BODIES = ["", "Return g.", "Line one\nLine two\n", "\n\nLeading blank lines"]
BRIEF_GATES = [
    (),
    (("uv", "run", "ruff", "check", "."),),
    (("pytest", "-k", "a b"), ("echo", "it's", "x")),
]
BRIEF_TESTS = [("tests/test_mod.py",), ("tests/a.py", "tests/b.py")]
BRIEF_ACCEPTANCE = [(), (("one", "tests/a.py::t1"), ("two", "tests/b.py::t2"))]
BRIEF_FILES = [(), ("pkg/mod.py",)]
BRIEF_SCOPE = [(), ("the CLI", "the HTTP API")]


def _brief_context(body, gates, tests, acceptance, files, scope) -> BriefContext:
    return BriefContext(
        task_key="WG-1",
        trial_id="T-1",
        repository="demo",
        worktree="/w/demo-wt",
        branch="agent/wg-1",
        subject="Add g",
        body=body,
        test_paths=tests,
        oracle=(("python", "-m", "pytest", "-q", *tests),),
        gates=gates,
        acceptance=acceptance,
        files=files,
        out_of_scope=scope,
    )


def _brief_error(template_id: str, context: BriefContext) -> dict:
    try:
        render(template_id, context)
    except ValidationFailed as exc:
        return {"error": type(exc).__name__, "message": str(exc)}
    return {"error": None}


def briefs_cases() -> list[dict]:
    base = BriefContext(
        task_key="WG-1",
        trial_id="T-1",
        repository="demo",
        worktree="/w/demo-wt",
        branch="agent/wg-1",
        subject="Add g",
        body="Return g.",
        test_paths=("tests/test_mod.py",),
        oracle=(("python", "-m", "pytest", "-q", "tests/test_mod.py"),),
        gates=(("uv", "run", "ruff", "check", "."),),
    )
    leaf = replace(
        base,
        acceptance=(("g returns two", "tests/test_mod.py::test_g"),),
        files=("pkg/mod.py", "tests/test_mod.py"),
        out_of_scope=("the CLI",),
    )
    cases = [
        {
            "id": "replay-golden",
            "covers": ["test_replay_v1_matches_golden_text"],
            "input": {"template": "replay-v1", "context": asdict(base)},
            "output": {"text": render("replay-v1", base)},
        },
        {
            "id": "replay-no-gates",
            "covers": ["test_no_gates_renders_none"],
            "input": {"template": "replay-v1", "context": asdict(replace(base, gates=()))},
            "output": {"text": render("replay-v1", replace(base, gates=()))},
        },
        {
            "id": "replay-empty-body",
            "covers": ["test_empty_body_is_followed_directly_by_next_section"],
            "input": {"template": "replay-v1", "context": asdict(replace(base, body=""))},
            "output": {"text": render("replay-v1", replace(base, body=""))},
        },
        {
            "id": "replay-unknown-template",
            "covers": ["test_unknown_template_is_rejected_naming_known_ids"],
            "input": {"template": "replay-v9", "context": asdict(base)},
            "output": _brief_error("replay-v9", base),
        },
        {
            "id": "replay-ignores-leaf-fields",
            "covers": ["test_replay_v1_ignores_leaf_only_fields"],
            "input": {"template": "replay-v1", "context": asdict(leaf)},
            "output": {"text": render("replay-v1", leaf)},
        },
        {
            "id": "rendered-ends-with-one-newline",
            "covers": ["test_rendered_output_ends_with_exactly_one_newline"],
            "input": {"template": "replay-v1", "context": asdict(base)},
            "output": {"text": render("replay-v1", base)},
        },
        {
            "id": "leaf-golden",
            "covers": ["test_leaf_v1_matches_golden_text"],
            "input": {"template": "leaf-v1", "context": asdict(leaf)},
            "output": {"text": render("leaf-v1", leaf)},
        },
        {
            "id": "leaf-empty-body-no-gates",
            "covers": ["test_leaf_v1_empty_body_and_no_gates"],
            "input": {"template": "leaf-v1", "context": asdict(replace(leaf, body="", gates=()))},
            "output": {"text": render("leaf-v1", replace(leaf, body="", gates=()))},
        },
        {
            "id": "leaf-empty-sections",
            "covers": ["test_leaf_v1_lists_none_when_a_section_is_empty"],
            "input": {
                "template": "leaf-v1",
                "context": asdict(replace(leaf, acceptance=(), out_of_scope=())),
            },
            "output": {"text": render("leaf-v1", replace(leaf, acceptance=(), out_of_scope=()))},
        },
    ]
    for body in BRIEF_BODIES:
        for gates in BRIEF_GATES:
            for tests in BRIEF_TESTS:
                for acceptance in BRIEF_ACCEPTANCE:
                    for files in BRIEF_FILES:
                        for scope in BRIEF_SCOPE:
                            context = _brief_context(body, gates, tests, acceptance, files, scope)
                            key = (
                                f"generated-b{BRIEF_BODIES.index(body)}-g{BRIEF_GATES.index(gates)}"
                                f"-t{BRIEF_TESTS.index(tests)}-a{BRIEF_ACCEPTANCE.index(acceptance)}"
                                f"-f{BRIEF_FILES.index(files)}-s{BRIEF_SCOPE.index(scope)}"
                            )
                            cases.append(
                                {
                                    "id": key,
                                    "covers": [],
                                    "input": asdict(context),
                                    "output": {
                                        "replay-v1": render("replay-v1", context),
                                        "leaf-v1": render("leaf-v1", context),
                                    },
                                }
                            )
    return cases


# ---------------------------------------------------------------- escape

ESCAPE_ROOT = r"C:\dev\rho"

NORMALIZE_INPUTS = [
    ("backslashes-lowercased", r"C:\Dev\Rho\src", ["test_backslashes_become_slashes_and_text_is_lowercased"]),
    ("msys-drive-in-command", "cd /c/dev/rho && git log", ["test_msys_drive_prefix_becomes_a_windows_drive"]),
    ("msys-drive-start", "/D/work", ["test_msys_drive_prefix_becomes_a_windows_drive"]),
    ("repeated-slashes", "c://dev///rho", ["test_repeated_slashes_collapse_before_the_drive_prefix_is_read"]),
    ("double-slash-drive", "//c/dev", ["test_repeated_slashes_collapse_before_the_drive_prefix_is_read"]),
    ("relative-drive-lookalike", "c:/c/dev", ["test_relative_segments_that_look_like_drives_are_left_alone"]),
    ("relative-path", "src/a/b", ["test_relative_segments_that_look_like_drives_are_left_alone"]),
    ("unicode", "C:\\Données\\Ü", []),
    ("empty", "", []),
]


def _escape_json(escapes) -> list[dict]:
    return [
        {"tool": e.tool, "tool_use_id": e.tool_use_id, "root": e.root, "excerpt": e.excerpt} for e in escapes
    ]


def escape_cases() -> list[dict]:
    cases = [
        {
            "id": f"normalize-{case_id}",
            "covers": covers,
            "input": {"text": text},
            "output": {"normalized": normalize_path_text(text)},
        }
        for case_id, text, covers in NORMALIZE_INPUTS
    ]
    find_cases = [
        (
            "sibling-prefix-no-match",
            [ToolCall("Read", {"file_path": "c:/dev/rho-other/x.rs"}, "t1")],
            [ESCAPE_ROOT],
            ["test_a_sibling_directory_with_the_root_as_prefix_does_not_match"],
        ),
        (
            "root-and-children",
            [
                ToolCall("Bash", {"command": v}, "t1")
                for v in ["c:/dev/rho", "c:/dev/rho/x", '"c:/dev/rho"', "cd /c/dev/rho && git log"]
            ],
            [ESCAPE_ROOT],
            ["test_the_root_itself_and_paths_under_it_match"],
        ),
        (
            "shell-operators",
            [
                ToolCall("Bash", {"command": v}, "t1")
                for v in ["c:/dev/rho;ls", "c:/dev/rho&&ls", "c:/dev/rho|cat", "c:/dev/rho\nls"]
            ],
            [ESCAPE_ROOT],
            ["test_root_followed_by_shell_operators_matches"],
        ),
        (
            "nested-values",
            [
                ToolCall(
                    "Edit",
                    {
                        "edits": [
                            {"old": "a", "new": "b"},
                            {"path": ["x", {"deep": r"C:\dev\rho\pkg\mod.py"}]},
                        ],
                        "meta": {"description": "unrelated"},
                    },
                    "t1",
                )
            ],
            [ESCAPE_ROOT],
            ["test_nested_dict_and_list_values_are_searched"],
        ),
        (
            "keys-and-non-strings",
            [ToolCall("Read", {"c:/dev/rho": 1, "n": 2, "flag": True}, "t1")],
            [ESCAPE_ROOT],
            ["test_keys_and_non_string_values_are_ignored"],
        ),
        (
            "one-per-call-and-root",
            [ToolCall("Read", {"a": "c:/dev/rho/x", "b": "c:/dev/rho/y", "c": "C:/dev/rho/other"}, "t1")],
            [ESCAPE_ROOT, "c:/dev/rho-other-clone/../rho"],
            ["test_one_escape_per_call_and_root"],
        ),
        (
            "nested-roots",
            [ToolCall("Read", {"a": "c:/dev/rho/x", "b": "c:/dev/rho/y", "c": "C:/dev/rho/other"}, "t9")],
            [r"C:\dev\rho", r"C:\dev\rho\other"],
            ["test_one_escape_per_call_and_root"],
        ),
        (
            "each-call-separately",
            [
                ToolCall("Read", {"f": "c:/dev/rho/a"}, "t1"),
                ToolCall("Read", {"f": "c:/other/a"}, "t2"),
            ],
            [ESCAPE_ROOT],
            ["test_each_call_is_checked_separately"],
        ),
        (
            "excerpt-cut",
            [ToolCall("Bash", {"command": "c:/dev/rho/" + "x" * 500}, "t1")],
            [ESCAPE_ROOT],
            ["test_excerpt_is_cut_to_300_characters"],
        ),
        (
            "empty-roots-ignored",
            [ToolCall("Bash", {"command": "anything"}, "t1")],
            ["", "/"],
            ["test_empty_roots_are_ignored"],
        ),
    ]
    for case_id, calls, roots, covers in find_cases:
        escapes = find_escapes(calls, roots)
        cases.append(
            {
                "id": f"find-{case_id}",
                "covers": covers,
                "input": {
                    "calls": [
                        {"name": c.name, "input": c.input, "tool_use_id": c.tool_use_id} for c in calls
                    ],
                    "roots": roots,
                },
                "output": {"escapes": _escape_json(escapes)},
            }
        )

    # Generated: every prefix x suffix combination against three roots.
    prefixes = [
        "c:/dev/rho",
        r"C:\dev\rho",
        "/c/dev/rho",
        "//c//dev//rho",
        "c:/dev/rho-other",
        "c:/dev/rhoX",
        "src/rho",
    ]
    suffixes = ["", "/x", ";ls", " ", '"', "&&ls", "|cat", "\nls", "rho", "/../rho"]
    roots = [ESCAPE_ROOT, "c:/dev/rho", "c:/dev"]
    for prefix in prefixes:
        for suffix in suffixes:
            text = prefix + suffix
            escapes = find_escapes([ToolCall("Bash", {"command": text}, "g1")], roots)
            cases.append(
                {
                    "id": f"generated-find-{prefix}-{suffix!r}",
                    "covers": [],
                    "input": {"text": text, "roots": roots},
                    "output": {"normalized": normalize_path_text(text), "escapes": _escape_json(escapes)},
                }
            )
    return cases


# ---------------------------------------------------------------- replay

MATCH_PATHS = [
    "test_x.py",
    "a/b/test_x.py",
    "tests/a/b.py",
    "src/tests/x.rs",
    "x/tests/y.rho",
    "pkg/foo_tests.rs",
    "Cargo.lock",
    "sub/Cargo.lock",
    "a/generated/x.rs",
    "a/b/generated/c.py",
    "x.py",
    "src/lib.rs",
]
MATCH_PATTERNS = [
    *replay_mod.IGNORE_GLOBS,
    *replay_mod.RUST_TEST_GLOBS,
    *replay_mod.PYTHON_TEST_GLOBS,
    "**/test_*.py",
    "*.py",
    "src/*.rs",
]

KIND_PATHS = [
    "src/lib.rs",
    "tests/x.rs",
    "README.md",
    "uv.lock",
    "a/generated/z.py",
    "tools/golden/x.py",
    "tests/test_a.py",
    "a/test_b.py",
    "Cargo.lock",
    "pkg/mod.rho",
    "tests/prog.rho",
    "a/b_tests.rs",
    "pkg/mod.py",
]

TREE = [
    "Cargo.toml",
    "crates/a/Cargo.toml",
    "crates/a/src/lib.rs",
    "crates/a/tests/t.rs",
    "crates/b/src/main.rs",
    "crates/b/src/x.rs",
    "pyproject.toml",
    "src/workgraph/mod.py",
    "tests/unit/test_mod.py",
    "web/package.json",
    "web/src/app.ts",
    "README.md",
    "uv.lock",
]

CHANGE_POOL = [
    "crates/a/src/lib.rs",
    "crates/a/tests/t.rs",
    "crates/b/src/main.rs",
    "crates/b/src/x.rs",
    "src/workgraph/mod.py",
    "tests/unit/test_mod.py",
    "web/src/app.ts",
    "README.md",
    "uv.lock",
    "crates/new/src/new.rs",
    "tests/unit/test_new.py",
]

PATCH_LINES = [
    "fn main() {}",
    "    #[test]",
    "#[cfg(test)]",
    "def test_x():",
    "    def test_y():",
    "x = 1",
    "todo!()",
    "raise NotImplementedError",
]

SHAPES = {
    "rust-only": ["Cargo.toml"],
    "python-only": ["pyproject.toml"],
    "rust-and-python": ["Cargo.toml", "pyproject.toml"],
    "neither": ["README.md"],
}


def _numstat(changes) -> str:
    out = []
    for status, path, old, added, removed in changes:
        if status[0] in "RC":
            out.append(f"{added}\t{removed}\t\0{old}\0{path}\0")
        else:
            out.append(f"{added}\t{removed}\t{path}\0")
    return "".join(out)


def _name_status(changes) -> str:
    out = []
    for status, path, old, _added, _removed in changes:
        if status[0] in "RC":
            out.append(f"{status}\0{old}\0{path}\0")
        else:
            out.append(f"{status}\0{path}\0")
    return "".join(out)


def _patch(changes, lines_by_path) -> str:
    chunks = []
    for _status, path, _old, _added, _removed in changes:
        lines = lines_by_path.get(path)
        if not lines:
            continue
        chunks.append(
            f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1,{len(lines)} @@\n"
            + "".join(f"+{line}\n" for line in lines)
        )
    return "".join(chunks)


def _generated_commits(count: int, seed: int) -> list[dict]:
    rng = Lcg(seed)
    commits = []
    for index in range(count):
        size = 1 + rng.next(6)
        changes = []
        used: set[str] = set()
        for _ in range(size):
            path = CHANGE_POOL[rng.next(len(CHANGE_POOL))]
            if path in used:
                continue
            used.add(path)
            kind = rng.next(10)
            if kind == 0:
                changes.append(("D", path, None, 3 + rng.next(9), 0))
            elif kind == 1:
                old = f"old/{path}"
                changes.append((f"R{80 + rng.next(20)}", path, old, rng.next(5), rng.next(5)))
            elif kind == 2:
                changes.append(("A", path, None, 1 + rng.next(60), 0))
            elif kind == 3:
                changes.append(("M", path, None, "-", "-"))  # binary: no line counts
            else:
                changes.append(("M", path, None, 1 + rng.next(60), rng.next(20)))
        lines_by_path = {}
        for status, path, _old, added, _removed in changes:
            if added == "-" or status[0] == "D":
                continue
            lines_by_path[path] = [PATCH_LINES[rng.next(len(PATCH_LINES))] for _ in range(min(added, 4))]
        commits.append({"index": index, "changes": changes, "lines_by_path": lines_by_path})
    return commits


def _config_for(files: list[str]) -> replay_mod.ReplayConfig:
    with tempfile.TemporaryDirectory(prefix="wg-golden-expmisc-") as scratch:
        root = Path(scratch)
        for name in files:
            (root / name).write_text("", encoding="utf-8", newline="\n")
        return replay_mod.ReplayConfig.for_repository(root)


def _replay_commit_output(commit: dict, config: replay_mod.ReplayConfig) -> dict:
    changes = commit["changes"]
    numstat = _numstat(changes)
    name_status = _name_status(changes)
    patch = _patch(changes, commit["lines_by_path"])
    parsed = replay_mod._parse_numstat(numstat)
    kinds = {c.path: replay_mod._kind(c.path, config) for c in parsed}
    sources = [c for c in parsed if kinds[c.path] == "source"]
    tests = [c for c in parsed if kinds[c.path] == "test"]
    src_lines = sum(c.lines for c in sources)
    manifests = replay_mod._manifest_dirs(frozenset(TREE), replay_mod.MANIFESTS)
    packages = sorted({replay_mod._nearest(c.path, manifests) or "" for c in sources})
    return {
        "changes": [
            {"path": c.path, "added": c.added, "removed": c.removed, "old_path": c.old_path} for c in parsed
        ],
        "kinds": kinds,
        "src_lines": src_lines,
        "test_lines": sum(c.lines for c in tests),
        "src_files": len(sources),
        "test_files": len(tests),
        "band": replay_mod._band(src_lines),
        "packages": packages,
        "has_inline_tests": replay_mod._has_inline_tests(replay_mod._added_lines(patch), sources),
        "tests_modified": list(replay_mod._tests_modified(name_status, config)),
        "changed_paths": list(replay_mod._changed_paths(name_status, config)),
    }


def replay_cases() -> list[dict]:
    cases: list[dict] = []

    matrix = [
        {"path": path, "pattern": pattern, "match": replay_mod.match_glob(path, pattern)}
        for path in MATCH_PATHS
        for pattern in MATCH_PATTERNS
    ]
    cases.append(
        {
            "id": "match-glob-matrix",
            "covers": [],
            "input": {"paths": MATCH_PATHS, "patterns": MATCH_PATTERNS},
            "output": {"matches": matrix},
        }
    )

    configs = {}
    for shape, manifests in SHAPES.items():
        config = _config_for(manifests)
        configs[shape] = config
        cases.append(
            {
                "id": f"config-{shape}",
                "covers": [],
                "input": {"manifests": manifests},
                "output": {
                    "test_globs": list(config.test_globs),
                    "source_suffixes": list(config.source_suffixes),
                    "ignore_globs": list(config.ignore_globs),
                },
            }
        )
        kinds = {path: replay_mod._kind(path, config) for path in KIND_PATHS}
        cases.append(
            {"id": f"kind-{shape}", "covers": [], "input": {"paths": KIND_PATHS}, "output": {"kinds": kinds}}
        )

    bands = [0, 1, 2, 3, 4, 7, 8, 1023, 1024, 2**20, 2**31 - 1, -5]
    cases.append(
        {
            "id": "band",
            "covers": [],
            "input": {"lines": bands},
            "output": {"bands": [replay_mod._band(v) for v in bands]},
        }
    )

    template_cases = [
        (["pytest", "-q", "{test_files}"], ["tests/a.py", "tests/b.py"], ["pkg"]),
        (["cargo", "test", "-p", "{packages}", "--", "{test_files}"], ["crates/a/tests/t.rs"], ["a", "b"]),
        (["uv", "run", "pytest"], [], []),
        (["{packages}"], ["x"], []),
    ]
    for template, tests, packages in template_cases:
        cases.append(
            {
                "id": f"expand-{'-'.join(template)}",
                "covers": [],
                "input": {"template": template, "test_paths": tests, "packages": packages},
                "output": {"argv": list(replay_mod.expand_oracle_template(template, tests, packages))},
            }
        )

    for path, dirs in [
        ("crates/a/src/lib.rs", ["", "crates/a", "crates/b"]),
        ("crates/zz/x.rs", ["", "crates/a"]),
        ("web/src/app.ts", ["web"]),
        ("README.md", []),
    ]:
        cases.append(
            {
                "id": f"nearest-{path}",
                "covers": [],
                "input": {"path": path, "dirs": dirs},
                "output": {"nearest": replay_mod._nearest(path, frozenset(dirs))},
            }
        )

    cases.append(
        {
            "id": "manifest-dirs-tree",
            "covers": [],
            "input": {"files": TREE, "names": list(replay_mod.MANIFESTS)},
            "output": {
                "dirs": sorted(replay_mod._manifest_dirs(frozenset(TREE), replay_mod.MANIFESTS)),
            },
        }
    )
    for test_paths in (["crates/a/tests/t.rs", "tests/unit/missing.py"], []):
        existing, packages = replay_mod._template_inputs(frozenset(TREE), test_paths)
        cases.append(
            {
                "id": f"template-inputs-{len(test_paths)}",
                "covers": [],
                "input": {"files": TREE, "test_paths": test_paths},
                "output": {"existing": list(existing), "packages": list(packages)},
            }
        )

    # Generated commit lists: each commit's numstat, name-status and zero-context patch, and what replay selection
    # reads from them, for a rust repo, a python repo, a mixed repo and a repo with neither manifest.
    for shape, manifests in SHAPES.items():
        for commit in _generated_commits(24, seed=20261025 + len(shape)):
            cases.append(
                {
                    "id": f"commit-{shape}-{commit['index']:02d}",
                    "covers": [],
                    "input": {
                        "shape": shape,
                        "manifests": manifests,
                        "numstat": _numstat(commit["changes"]),
                        "name_status": _name_status(commit["changes"]),
                        "patch": _patch(commit["changes"], commit["lines_by_path"]),
                    },
                    "output": _replay_commit_output(commit, configs[shape]),
                }
            )
    return cases


def main() -> None:
    write("contract", contract_cases())
    write("briefs", briefs_cases())
    write("escape", escape_cases())
    write("replay", replay_cases())


if __name__ == "__main__":
    main()
