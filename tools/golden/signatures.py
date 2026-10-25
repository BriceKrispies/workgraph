"""Golden vectors for failure-signature extraction and regression judging (GV-02).

Writes golden/signatures/extract.json and golden/signatures/regression.json. Every expected value
is computed by the Python implementation (workgraph.verification.service.extract_failure_signatures
and workgraph.integrations.git.regression.regressions); the script additionally checks that the
logs and check fixtures taken from tests/unit reproduce the assertions written in those tests.

Usage: uv run python tools/golden/signatures.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT))

from tests.unit import test_failure_signatures as tfs  # noqa: E402
from tests.unit import test_regression as tr  # noqa: E402

from workgraph.integrations.git.regression import CheckOutcome, regressions  # noqa: E402
from workgraph.verification.service import extract_failure_signatures  # noqa: E402

OUT = ROOT / "golden" / "signatures"
PARSERS = ("auto", "cargo", "pytest", "generic")


def _write(name: str, payload: Any) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    with (OUT / name).open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


# ---------------------------------------------------------------------------
# Extraction cases
# ---------------------------------------------------------------------------


def _extract_case(
    case_id: str, source: str, lines: list[str], asserted: dict[str, list[str]] | None = None
) -> dict:
    expected = {parser: extract_failure_signatures(lines, parser) for parser in PARSERS}
    for parser, value in (asserted or {}).items():
        if expected[parser] != value:
            raise SystemExit(f"{case_id}: {parser} gives {expected[parser]!r}, test asserts {value!r}")
    return {"id": case_id, "source": source, "lines": lines, "expected": expected}


def _cargo_log(binaries: list[dict[str, Any]], summary: list[str] | None = None) -> list[str]:
    """A cargo test log: one block per binary, optional crash, optional build failure, optional summary."""
    lines: list[str] = []
    for b in binaries:
        if b.get("doc"):
            lines.append(f"   Doc-tests {b['name']}")
        else:
            sep = "\\" if b.get("windows") else "/"
            lines.append(
                f"     Running unittests src/lib.rs (target{sep}debug{sep}deps{sep}{b['name']}-{b['hash']}"
                f"{'.exe' if b.get('windows') else ''})"
            )
        lines.append("")
        lines.append(f"running {len(b['tests'])} tests")
        for name, status in b["tests"]:
            if status == "fail":
                lines.append(f"test {name} ... FAILED")
            elif status == "ignore":
                lines.append(f"test {name} ... ignored")
            else:
                lines.append(f"test {name} ... ok")
        if b.get("crash"):
            lines.append(f"error: test failed, to rerun pass `{b['rerun']}`")
        lines.append("")
    for crate in summary or []:
        lines.append(f"error: could not compile `{crate}` due to 1 previous error")
    return lines


def extract_cases() -> list[dict]:
    cases: list[dict] = []

    # Cases taken from tests/unit/test_failure_signatures.py, with the asserted values checked.
    cases.append(
        _extract_case(
            "test-cargo-binaries-crash-build",
            "tests/unit/test_failure_signatures.py::test_cargo_failures_across_binaries_crash_and_build_error",
            tfs.CARGO_RUN.splitlines(),
            {
                "cargo": [
                    "test:rho_lexer::scan::strings::unterminated",
                    "test:one_version::sorted_once",
                    "binary:courtyard_gpu",
                    "build:rho-lowering",
                ],
                "auto": [
                    "test:rho_lexer::scan::strings::unterminated",
                    "test:one_version::sorted_once",
                    "binary:courtyard_gpu",
                    "build:rho-lowering",
                ],
            },
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-ok-ignored-bench",
            "tests/unit/test_failure_signatures.py::test_cargo_ok_and_ignored_lines_produce_nothing",
            [
                "     Running unittests src/lib.rs (target/debug/deps/rho_lexer-0a1b2c3d4e5f6789)",
                "test scan::numbers ... ok",
                "test scan::idents ... ignored",
                "test bench_scan ... bench:  1,234 ns/iter (+/- 5)",
                "test result: ok. 1 passed; 0 failed; 1 ignored",
            ],
            {"cargo": []},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-crash-suppressed-by-test-failure",
            "tests/unit/test_failure_signatures.py::test_cargo_binary_crash_is_not_reported_when_tests_failed",
            [
                "     Running tests/one_version.rs (target/debug/deps/one_version-0123456789abcdef)",
                "test sorted_once ... FAILED",
                "error: test failed, to rerun pass `--test one_version`",
            ],
            {"cargo": ["test:one_version::sorted_once"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-no-fail-fast-summary",
            "tests/unit/test_failure_signatures.py::test_cargo_no_fail_fast_summary_adds_binary_for_crashed_target",
            tfs.NO_FAIL_FAST_SUMMARY.splitlines(),
            {"cargo": ["test:rho_lowering::lower::calls", "binary:courtyard_gpu"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-summary-singular-targets",
            "tests/unit/test_failure_signatures.py::test_cargo_summary_singular_form_and_target_names",
            [
                "     Running unittests src/main.rs (target/debug/deps/rho_gpu-0123456789abcdef)",
                "test ok_case ... ok",
                "error: 1 target failed:",
                "    `-p rho-gpu --bin rho-gpu`",
                "    `-p rho-lowering --doc`",
                "    `-p rho-gpu --example demo-app`",
            ],
            {"cargo": ["binary:rho_gpu", "binary:rho_lowering", "binary:demo_app"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-summary-skips-reported",
            "tests/unit/test_failure_signatures.py::test_cargo_summary_skips_targets_already_reported_with_test_signatures",
            [
                "     Running unittests src/lib.rs (target/debug/deps/rho-0123456789abcdef)",
                "test a::b ... FAILED",
                "error: 1 target failed:",
                "    `-p rho --lib`",
            ],
            {"cargo": ["test:rho::a::b"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-name-with-spaces",
            "tests/unit/test_failure_signatures.py::test_cargo_test_name_with_spaces_is_kept_whole",
            ["   Doc-tests rho_lexer", "test src/lib.rs - scan (line 12) ... FAILED"],
            {"cargo": ["test:rho_lexer::src/lib.rs - scan (line 12)"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-dedup-first-appearance",
            "tests/unit/test_failure_signatures.py::test_cargo_signatures_are_deduplicated_in_first_appearance_order",
            [
                "     Running unittests src/lib.rs (target/debug/deps/a-0123456789abcdef)",
                "test x::one ... FAILED",
                "test x::two ... FAILED",
                "test x::one ... FAILED",
                "error: could not compile `b` due to 1 previous error",
                "error: could not compile `b` due to 1 previous error",
            ],
            {"cargo": ["test:a::x::one", "test:a::x::two", "build:b"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-no-cap",
            "tests/unit/test_failure_signatures.py::test_cargo_has_no_signature_cap",
            [f"test t::n{i} ... FAILED" for i in range(12)],
            {"cargo": [f"test:t::n{i}" for i in range(12)]},
        )
    )
    cases.append(
        _extract_case(
            "test-pytest-line",
            "tests/unit/test_failure_signatures.py::test_auto_falls_back_to_pytest_then_generic",
            ["FAILED tests/test_api.py::test_login - assert 1 == 2"],
            {"auto": ["test:tests/test_api.py::test_login"]},
        )
    )
    cases.append(
        _extract_case(
            "test-generic-lines",
            "tests/unit/test_failure_signatures.py::test_auto_falls_back_to_pytest_then_generic",
            ["something went wrong", "Error: disk full"],
            {"auto": ["error:disk full"]},
        )
    )
    cases.append(
        _extract_case(
            "test-cargo-parser-no-generic",
            "tests/unit/test_failure_signatures.py::test_cargo_parser_does_not_fall_back_to_generic",
            ["Error: disk full"],
            {"cargo": []},
        )
    )

    # Generated cargo logs: several binaries, Windows and Unix paths, crashes, build failures.
    cases.append(
        _extract_case(
            "gen-cargo-five-binaries",
            "generated: five binaries, mixed path styles, one crash, one build failure",
            _cargo_log(
                [
                    {
                        "name": "alpha",
                        "hash": "0123456789abcdef",
                        "windows": True,
                        "tests": [("a::one", "fail"), ("a::two", "ok")],
                    },
                    {
                        "name": "beta",
                        "hash": "fedcba9876543210",
                        "tests": [("b::ok", "ok")],
                        "crash": True,
                        "rerun": "--test beta",
                    },
                    {
                        "name": "gamma",
                        "hash": "00ff00ff00ff00ff",
                        "windows": True,
                        "tests": [("g::x", "ignore"), ("g::y", "fail"), ("g::z", "fail")],
                    },
                    {"name": "delta", "hash": "1111222233334444", "tests": [("d::only", "ok")]},
                    {"name": "eps", "hash": "abcdefabcdefabcd", "tests": [("e::fail", "fail")]},
                ],
                summary=["rho-broken"],
            ),
        )
    )
    cases.append(
        _extract_case(
            "gen-cargo-doc-tests",
            "generated: doc-tests block with failing example and a binary-level crash",
            _cargo_log(
                [
                    {"name": "rho_core", "hash": "aaaaaaaaaaaaaaaa", "tests": [("ok_one", "ok")]},
                    {"name": "rho_core", "doc": True, "tests": [("src/lib.rs - parse (line 4)", "fail")]},
                    {
                        "name": "rho_cli",
                        "hash": "bbbbbbbbbbbbbbbb",
                        "tests": [("cli::run", "ok")],
                        "crash": True,
                        "rerun": "--test rho_cli",
                    },
                ]
            ),
        )
    )
    cases.append(
        _extract_case(
            "gen-cargo-summary-targets",
            "generated: N-targets summary with lib, bin, example, bench, doc and an unparseable target",
            [
                "     Running unittests src/lib.rs (target/debug/deps/rho_lib-0123456789abcdef)",
                "test ok ... ok",
                "error: 6 targets failed:",
                "    `-p rho-lib --lib`",
                "    `-p rho-cli --bin rho`",
                "    `-p rho-cli --example demo_app`",
                "    `-p rho-bench --bench speed`",
                "    `-p rho-doc --doc`",
                "    `-p rho-weird`",
            ],
            {"cargo": ["binary:rho_lib", "binary:rho", "binary:demo_app", "binary:speed", "binary:rho_doc"]},
        )
    )
    cases.append(
        _extract_case(
            "gen-cargo-build-failures",
            "generated: several crates that do not compile, each reported twice",
            [
                "error: could not compile `rho-lexer` (lib) due to 2 previous errors",
                "error: could not compile `rho-parser` (lib test) due to 1 previous error",
                "error: could not compile `rho-lexer` (lib) due to 2 previous errors",
                "warning: unused variable `x`",
            ],
        )
    )
    cases.append(
        _extract_case(
            "gen-cargo-unicode-and-quotes",
            "generated: non-ASCII test names and a crash-free run",
            _cargo_log(
                [
                    {
                        "name": "uni",
                        "hash": "cccccccccccccccc",
                        "tests": [("名前::テスト", "fail"), ("ok_é", "ok")],
                    },
                ]
            ),
        )
    )

    # Generated pytest logs.
    cases.append(
        _extract_case(
            "gen-pytest-mixed",
            "generated: pytest short summary with FAILED and ERROR lines and indentation",
            [
                "============================= test session starts =============================",
                "collected 9 items",
                "tests/test_api.py F.....E..",
                "=========================== short test summary info ===========================",
                "FAILED tests/test_api.py::test_login - assert 1 == 2",
                "ERROR tests/test_db.py::TestDb::test_setup - RuntimeError: no db",
                "   FAILED tests/test_api.py::test_logout - indented line",
                "FAILED tests/test_api.py::test_login - duplicate line",
                "not a failure line with FAILED inside",
            ],
        )
    )
    cases.append(
        _extract_case(
            "gen-pytest-none",
            "generated: pytest run that passed",
            ["collected 3 items", "3 passed in 0.12s"],
        )
    )

    # Generated generic logs: window of the last 50 lines, the cap of five, truncation at 120 characters.
    long_message = "x" * 200
    cases.append(
        _extract_case(
            "gen-generic-window-cap-truncate",
            "generated: generic matches before the last 50 lines are ignored, cap of five, 120-char truncation",
            ["Error: early line outside the window"]
            + [f"Error: earlier {i}" for i in range(10)]
            + [f"noise {i}" for i in range(40)]
            + [f"FAILURE: last block {i}" for i in range(7)]
            + [f"Failed: {long_message}"],
        )
    )
    cases.append(
        _extract_case(
            "gen-generic-patterns",
            "generated: error, failed, failure in several positions and cases",
            [
                "ERROR: upper case",
                "the build failed with status",
                "failure:colon no space",
                "errors are not matched",
                "a failed\tcolon-tab",
                "Error:   padded value   ",
            ],
        )
    )
    cases.append(
        _extract_case(
            "gen-empty",
            "generated: empty log",
            [],
        )
    )
    return cases


# ---------------------------------------------------------------------------
# Regression cases
# ---------------------------------------------------------------------------


def _regression_case(
    case_id: str,
    source: str,
    checks: list[dict[str, Any]],
    specs: list[dict[str, str]],
    base: dict[str, dict[str, Any]] | None,
    asserted: dict[str, list[str]] | None = None,
) -> dict:
    run = tr._run(
        *[
            tr._check(
                c["name"],
                status=c.get("status", "failed"),
                signatures=c.get("signatures"),
                required=c.get("required", True),
            )
            for c in checks
        ]
    )
    spec_map = tr._specs(*[tr._spec(s["name"], s["baseline"]) for s in specs])
    base_map = None
    if base is not None:
        base_map = {
            name: CheckOutcome(o["status"], list(o.get("signatures", [])), o.get("run_id"))
            for name, o in base.items()
        }
    expected = regressions(run, base_map, spec_map)
    if asserted is not None and expected != asserted:
        raise SystemExit(f"{case_id}: gives {expected!r}, test asserts {asserted!r}")
    return {
        "id": case_id,
        "source": source,
        "checks": checks,
        "specs": specs,
        "base": base,
        "base_is_none": base is None,
        "expected": expected,
    }


def regression_cases() -> list[dict]:
    t = "tests/unit/test_regression.py::"
    cases: list[dict] = []
    R, A = "regression", "absolute"
    cases.append(
        _regression_case(
            "passed-never-failing",
            t + "test_a_passed_check_is_never_failing",
            [{"name": "suite", "status": "passed"}],
            [{"name": "suite", "baseline": R}],
            {},
            {},
        )
    )
    cases.append(
        _regression_case(
            "optional-failing",
            t + "test_an_optional_failing_check_is_not_failing",
            [{"name": "lint", "signatures": ["test:x"], "required": False}],
            [{"name": "lint", "baseline": A}],
            None,
            {},
        )
    )
    cases.append(
        _regression_case(
            "absolute-failure",
            t + "test_an_absolute_failure_is_failing_whatever_the_base_says",
            [{"name": "suite", "signatures": ["test:a", "test:b"]}],
            [{"name": "suite", "baseline": A}],
            {"suite": {"status": "failed", "signatures": ["test:a", "test:b"]}},
            {"suite": ["test:a", "test:b"]},
        )
    )
    cases.append(
        _regression_case(
            "known-failure-tolerated",
            t + "test_a_regression_failure_already_on_the_base_is_tolerated",
            [{"name": "suite", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": ["test:a"]}},
            {},
        )
    )
    cases.append(
        _regression_case(
            "moved-test-tolerated",
            t + "test_a_known_failure_whose_test_moved_to_another_binary_is_tolerated",
            [{"name": "suite", "signatures": ["test:spec02::spec02_055::a_device_run", "test:fresh"]}],
            [{"name": "suite", "baseline": R}],
            {
                "suite": {
                    "status": "failed",
                    "signatures": ["test:a_device_run", "test:spec02_055::a_device_run"],
                }
            },
            {"suite": ["test:fresh"]},
        )
    )
    cases.append(
        _regression_case(
            "only-new-in-order",
            t + "test_only_the_new_signatures_of_a_regression_failure_are_reported_in_order",
            [{"name": "suite", "signatures": ["test:c", "test:a", "test:b"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": ["test:a"]}},
            {"suite": ["test:c", "test:b"]},
        )
    )
    cases.append(
        _regression_case(
            "no-outcome-failing",
            t + "test_a_regression_failure_is_failing_when_the_base_has_no_outcome",
            [{"name": "suite", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            {},
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "no-outcome-none-base",
            t + "test_a_regression_failure_is_failing_when_the_base_has_no_outcome",
            [{"name": "suite", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            None,
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "base-passed-failing",
            t + "test_a_regression_failure_is_failing_when_the_base_passed",
            [{"name": "suite", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "passed"}},
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "timeout-failing",
            t + "test_a_timeout_is_failing_even_when_the_base_fails_the_same_way",
            [{"name": "suite", "signatures": ["timeout", "test:a"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": ["timeout", "test:a"]}},
            {"suite": ["timeout", "test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "unparseable-failing",
            t + "test_an_unparseable_failure_is_failing_even_when_the_base_fails",
            [{"name": "suite", "signatures": []}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": []}},
            {"suite": []},
        )
    )
    cases.append(
        _regression_case(
            "judged-by-own-mode",
            t + "test_checks_are_judged_by_their_own_mode",
            [
                {"name": "unit", "signatures": ["test:a"]},
                {"name": "integration", "signatures": ["test:b"]},
                {"name": "types", "signatures": ["error:x"]},
            ],
            [
                {"name": "unit", "baseline": R},
                {"name": "integration", "baseline": A},
                {"name": "types", "baseline": A},
            ],
            {
                "unit": {"status": "failed", "signatures": ["test:a"]},
                "integration": {"status": "failed", "signatures": ["test:b"]},
            },
            {"integration": ["test:b"], "types": ["error:x"]},
        )
    )
    cases.append(
        _regression_case(
            "missing-spec-absolute",
            t + "test_a_check_missing_from_the_specs_is_judged_absolutely",
            [{"name": "suite", "signatures": ["test:a"]}],
            [],
            {"suite": {"status": "failed", "signatures": ["test:a"]}},
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "errored-absolute",
            t + "test_an_errored_check_is_failing_in_absolute_mode_whatever_the_base_says",
            [{"name": "suite", "status": "error"}],
            [{"name": "suite", "baseline": A}],
            {"suite": {"status": "error", "run_id": "older"}},
            {"suite": []},
        )
    )
    cases.append(
        _regression_case(
            "errored-never-tolerated",
            t + "test_an_errored_check_is_never_tolerated_in_regression_mode",
            [{"name": "suite", "status": "error", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": ["test:a"]}},
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "errored-never-tolerated-no-base",
            t + "test_an_errored_check_is_never_tolerated_in_regression_mode",
            [{"name": "suite", "status": "error", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            None,
            {"suite": ["test:a"]},
        )
    )
    cases.append(
        _regression_case(
            "errored-optional",
            t + "test_an_errored_optional_check_is_not_failing",
            [{"name": "lint", "status": "error", "required": False}],
            [{"name": "lint", "baseline": A}],
            None,
            {},
        )
    )
    cases.append(
        _regression_case(
            "run-id-ignored",
            t + "test_outcome_run_id_does_not_affect_the_judgement",
            [{"name": "suite", "signatures": ["test:a"]}],
            [{"name": "suite", "baseline": R}],
            {"suite": {"status": "failed", "signatures": ["test:a"], "run_id": "older"}},
            {},
        )
    )

    # Generated: the leaf rule on several shapes, and a mixed run.
    cases.append(
        _regression_case(
            "gen-leaf-shapes",
            "generated: base leaf matches across module depth",
            [
                {
                    "name": "suite",
                    "signatures": [
                        "test:a::b::c::deep",
                        "test:c::deep",
                        "test:x::y",
                        "test:only_here",
                        "binary:rho",
                        "build:rho-lexer",
                        "error:boom",
                    ],
                }
            ],
            [{"name": "suite", "baseline": R}],
            {
                "suite": {
                    "status": "failed",
                    "signatures": ["test:deep", "test:z", "binary:rho", "error:boom"],
                }
            },
            {"suite": ["test:x::y", "test:only_here", "build:rho-lexer"]},
        )
    )
    cases.append(
        _regression_case(
            "gen-mixed-run",
            "generated: several checks, each in a different mode and status",
            [
                {"name": "unit", "signatures": ["test:a", "test:new"]},
                {"name": "e2e", "signatures": ["timeout"]},
                {"name": "types", "signatures": ["error:t"]},
                {"name": "lint", "status": "passed"},
                {"name": "stack", "status": "error"},
                {"name": "docs", "signatures": ["test:d"], "required": False},
            ],
            [
                {"name": "unit", "baseline": R},
                {"name": "e2e", "baseline": R},
                {"name": "types", "baseline": R},
                {"name": "lint", "baseline": R},
                {"name": "stack", "baseline": R},
                {"name": "docs", "baseline": A},
            ],
            {
                "unit": {"status": "failed", "signatures": ["test:a"]},
                "e2e": {"status": "failed", "signatures": ["timeout"]},
                "types": {"status": "passed"},
                "lint": {"status": "failed", "signatures": []},
                "stack": {"status": "failed"},
            },
        )
    )
    cases.append(_regression_case("gen-empty-run", "generated: run with no checks", [], [], None, {}))
    return cases


def main() -> None:
    _write("extract.json", {"cases": extract_cases(), "parsers": list(PARSERS)})
    _write("regression.json", {"cases": regression_cases()})
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
