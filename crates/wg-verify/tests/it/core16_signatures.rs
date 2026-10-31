//! Port of `tests/unit/test_failure_signatures.py`.
#![allow(clippy::expect_used)]

use wg_verify::{FailureParser, extract_failure_signatures};

const CARGO_RUN: &str = r"
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
";

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn cargo(lines: &[&str]) -> Vec<String> {
    extract_failure_signatures(lines, FailureParser::Cargo)
}

#[test]
fn cargo_failures_across_binaries_crash_and_build_error() {
    assert_eq!(
        cargo(&lines(CARGO_RUN)),
        [
            "test:rho_lexer::scan::strings::unterminated",
            "test:one_version::sorted_once",
            "binary:courtyard_gpu",
            "build:rho-lowering",
        ]
    );
}

#[test]
fn cargo_ok_and_ignored_lines_produce_nothing() {
    let lines = [
        "     Running unittests src/lib.rs (target/debug/deps/rho_lexer-0a1b2c3d4e5f6789)",
        "test scan::numbers ... ok",
        "test scan::idents ... ignored",
        "test bench_scan ... bench:  1,234 ns/iter (+/- 5)",
        "test result: ok. 1 passed; 0 failed; 1 ignored",
    ];
    assert!(cargo(&lines).is_empty());
}

#[test]
fn cargo_binary_crash_is_not_reported_when_tests_failed() {
    let lines = [
        "     Running tests/one_version.rs (target/debug/deps/one_version-0123456789abcdef)",
        "test sorted_once ... FAILED",
        "error: test failed, to rerun pass `--test one_version`",
    ];
    assert_eq!(cargo(&lines), ["test:one_version::sorted_once"]);
}

const NO_FAIL_FAST_SUMMARY: &str = r"
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
";

#[test]
fn cargo_no_fail_fast_summary_adds_binary_for_crashed_target() {
    assert_eq!(
        cargo(&lines(NO_FAIL_FAST_SUMMARY)),
        ["test:rho_lowering::lower::calls", "binary:courtyard_gpu"]
    );
}

#[test]
fn cargo_summary_singular_form_and_target_names() {
    let lines = [
        "     Running unittests src/main.rs (target/debug/deps/rho_gpu-0123456789abcdef)",
        "test ok_case ... ok",
        "error: 1 target failed:",
        "    `-p rho-gpu --bin rho-gpu`",
        "    `-p rho-lowering --doc`",
        "    `-p rho-gpu --example demo-app`",
    ];
    assert_eq!(
        cargo(&lines),
        ["binary:rho_gpu", "binary:rho_lowering", "binary:demo_app"]
    );
}

#[test]
fn cargo_summary_skips_targets_already_reported_with_test_signatures() {
    let lines = [
        "     Running unittests src/lib.rs (target/debug/deps/rho-0123456789abcdef)",
        "test a::b ... FAILED",
        "error: 1 target failed:",
        "    `-p rho --lib`",
    ];
    assert_eq!(cargo(&lines), ["test:rho::a::b"]);
}

#[test]
fn cargo_test_name_with_spaces_is_kept_whole() {
    let lines = [
        "   Doc-tests rho_lexer",
        "test src/lib.rs - scan (line 12) ... FAILED",
    ];
    assert_eq!(
        cargo(&lines),
        ["test:rho_lexer::src/lib.rs - scan (line 12)"]
    );
}

#[test]
fn cargo_signatures_are_deduplicated_in_first_appearance_order() {
    let lines = [
        "     Running unittests src/lib.rs (target/debug/deps/a-0123456789abcdef)",
        "test x::one ... FAILED",
        "test x::two ... FAILED",
        "test x::one ... FAILED",
        "error: could not compile `b` due to 1 previous error",
        "error: could not compile `b` due to 1 previous error",
    ];
    assert_eq!(
        cargo(&lines),
        ["test:a::x::one", "test:a::x::two", "build:b"]
    );
}

#[test]
fn cargo_has_no_signature_cap() {
    let lines: Vec<String> = (0..12)
        .map(|i| format!("test t::n{i} ... FAILED"))
        .collect();
    assert_eq!(
        extract_failure_signatures(&lines, FailureParser::Cargo).len(),
        12
    );
}

#[test]
fn auto_picks_cargo_signatures() {
    let first = extract_failure_signatures(&lines(CARGO_RUN), FailureParser::Auto)
        .into_iter()
        .next();
    assert_eq!(
        first.as_deref(),
        Some("test:rho_lexer::scan::strings::unterminated")
    );
}

#[test]
fn auto_falls_back_to_pytest_then_generic() {
    let pytest_lines = ["FAILED tests/test_api.py::test_login - assert 1 == 2"];
    assert_eq!(
        extract_failure_signatures(&pytest_lines, FailureParser::Auto),
        ["test:tests/test_api.py::test_login"]
    );
    let generic_lines = ["something went wrong", "Error: disk full"];
    assert_eq!(
        extract_failure_signatures(&generic_lines, FailureParser::Auto),
        ["error:disk full"]
    );
}

#[test]
fn cargo_parser_does_not_fall_back_to_generic() {
    let lines = ["Error: disk full"];
    assert!(extract_failure_signatures(&lines, FailureParser::Cargo).is_empty());
}
