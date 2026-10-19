from __future__ import annotations

from workgraph.verification.service import extract_failure_signatures

# Two test binaries with failures (Windows-style path for the first), a binary that dies
# without per-test results, and a crate that does not compile.
CARGO_RUN = r"""
   Compiling rho-lexer v0.3.0 (C:\dev\rho\crates\lexer)
     Running unittests src/lib.rs (target\debug\deps\rho_lexer-0a1b2c3d4e5f6789.exe)

running 4 tests
test scan::numbers ... ok
test scan::strings::unterminated ... FAILED
test scan::idents ... ignored
test scan::spans ... ok

failures:

---- scan::strings::unterminated stdout ----
thread 'scan::strings::unterminated' panicked at src/scan.rs:88:9:
assertion failed

failures:
    scan::strings::unterminated

test result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `-p rho-lexer --lib`

     Running tests/one_version.rs (target/debug/deps/one_version-0123456789abcdef)

running 2 tests
test sorted_once ... FAILED
test merged ... ok

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

error: test failed, to rerun pass `--test one_version`

     Running tests/courtyard_gpu.rs (target/debug/deps/courtyard_gpu-fedcba9876543210)

running 1 test
test upload::small ... ok
error: test failed, to rerun pass `--test courtyard_gpu`

error: could not compile `rho-lowering` (lib test) due to 3 previous errors
"""


def test_cargo_failures_across_binaries_crash_and_build_error():
    lines = CARGO_RUN.splitlines()
    assert extract_failure_signatures(lines, "cargo") == [
        "test:rho_lexer::scan::strings::unterminated",
        "test:one_version::sorted_once",
        "binary:courtyard_gpu",
        "build:rho-lowering",
    ]


def test_cargo_ok_and_ignored_lines_produce_nothing():
    lines = [
        "     Running unittests src/lib.rs (target/debug/deps/rho_lexer-0a1b2c3d4e5f6789)",
        "test scan::numbers ... ok",
        "test scan::idents ... ignored",
        "test bench_scan ... bench:  1,234 ns/iter (+/- 5)",
        "test result: ok. 1 passed; 0 failed; 1 ignored",
    ]
    assert extract_failure_signatures(lines, "cargo") == []


def test_cargo_binary_crash_is_not_reported_when_tests_failed():
    lines = [
        "     Running tests/one_version.rs (target/debug/deps/one_version-0123456789abcdef)",
        "test sorted_once ... FAILED",
        "error: test failed, to rerun pass `--test one_version`",
    ]
    assert extract_failure_signatures(lines, "cargo") == ["test:one_version::sorted_once"]


NO_FAIL_FAST_SUMMARY = r"""
     Running unittests src/lib.rs (target\debug\deps\rho_lowering-0a1b2c3d4e5f6789.exe)

running 1 test
test lower::calls ... FAILED

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `-p rho-lowering --lib`

     Running tests/courtyard_gpu.rs (target/debug/deps/courtyard_gpu-fedcba9876543210)

running 1 test
test upload::small ... ok
error: 2 targets failed:
    `-p rho-lowering --lib`
    `-p rho --test courtyard_gpu`
"""


def test_cargo_no_fail_fast_summary_adds_binary_for_crashed_target():
    lines = NO_FAIL_FAST_SUMMARY.splitlines()
    assert extract_failure_signatures(lines, "cargo") == [
        "test:rho_lowering::lower::calls",
        "binary:courtyard_gpu",
    ]


def test_cargo_summary_singular_form_and_target_names():
    lines = [
        "     Running unittests src/main.rs (target/debug/deps/rho_gpu-0123456789abcdef)",
        "test ok_case ... ok",
        "error: 1 target failed:",
        "    `-p rho-gpu --bin rho-gpu`",
        "    `-p rho-lowering --doc`",
        "    `-p rho-gpu --example demo-app`",
    ]
    assert extract_failure_signatures(lines, "cargo") == [
        "binary:rho_gpu",
        "binary:rho_lowering",
        "binary:demo_app",
    ]


def test_cargo_summary_skips_targets_already_reported_with_test_signatures():
    lines = [
        "     Running unittests src/lib.rs (target/debug/deps/rho-0123456789abcdef)",
        "test a::b ... FAILED",
        "error: 1 target failed:",
        "    `-p rho --lib`",
    ]
    assert extract_failure_signatures(lines, "cargo") == ["test:rho::a::b"]


def test_cargo_test_name_with_spaces_is_kept_whole():
    lines = [
        "   Doc-tests rho_lexer",
        "test src/lib.rs - scan (line 12) ... FAILED",
    ]
    assert extract_failure_signatures(lines, "cargo") == ["test:rho_lexer::src/lib.rs - scan (line 12)"]


def test_cargo_signatures_are_deduplicated_in_first_appearance_order():
    lines = [
        "     Running unittests src/lib.rs (target/debug/deps/a-0123456789abcdef)",
        "test x::one ... FAILED",
        "test x::two ... FAILED",
        "test x::one ... FAILED",
        "error: could not compile `b` due to 1 previous error",
        "error: could not compile `b` due to 1 previous error",
    ]
    assert extract_failure_signatures(lines, "cargo") == [
        "test:a::x::one",
        "test:a::x::two",
        "build:b",
    ]


def test_cargo_has_no_signature_cap():
    lines = [f"test t::n{i} ... FAILED" for i in range(12)]
    assert len(extract_failure_signatures(lines, "cargo")) == 12


def test_auto_picks_cargo_signatures():
    assert extract_failure_signatures(CARGO_RUN.splitlines(), "auto")[0] == (
        "test:rho_lexer::scan::strings::unterminated"
    )


def test_auto_falls_back_to_pytest_then_generic():
    pytest_lines = ["FAILED tests/test_api.py::test_login - assert 1 == 2"]
    assert extract_failure_signatures(pytest_lines, "auto") == ["test:tests/test_api.py::test_login"]
    generic_lines = ["something went wrong", "Error: disk full"]
    assert extract_failure_signatures(generic_lines, "auto") == ["error:disk full"]


def test_cargo_parser_does_not_fall_back_to_generic():
    assert extract_failure_signatures(["Error: disk full"], "cargo") == []
