//! Failure signatures: short, stable strings that name what a failed check failed on, extracted
//! from its log. Port of `extract_failure_signatures` and the cargo parser in
//! `verification/service.py`; the golden vectors of GV-02 (`golden/signatures/extract.json`) pin
//! the behaviour.
//!
//! Signature forms: `test:<binary>::<name>` (or `test:<name>` without a binary header),
//! `binary:<name>` for a test binary that died without per-test results, `build:<crate>` for a
//! crate that does not compile, `test:<id>` for pytest, and `error:<text>` for the generic
//! fallback.

use regex::Regex;
use rpds::{RedBlackTreeSetSync, VectorSync};
use wg_types::pyval::prefix;

/// Which parser reads a check's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureParser {
    /// Cargo first, then pytest, then the generic error scan.
    Auto,
    Pytest,
    Generic,
    Cargo,
}

impl FailureParser {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pytest => "pytest",
            Self::Generic => "generic",
            Self::Cargo => "cargo",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "pytest" => Some(Self::Pytest),
            "generic" => Some(Self::Generic),
            "cargo" => Some(Self::Cargo),
            _ => None,
        }
    }
}

/// The compiled patterns of one extraction. Patterns are compiled where they are used (no
/// `LazyLock`, which the Core tier forbids); the constants are checked by the GV-02 test.
struct Patterns {
    pytest_failed: Regex,
    generic_error: Regex,
    cargo_running: Regex,
    cargo_doc_tests: Regex,
    cargo_test_failed: Regex,
    cargo_binary_died: Regex,
    cargo_build_failed: Regex,
    cargo_targets_failed: Regex,
    cargo_target_line: Regex,
    binary_hash: Regex,
}

fn compile(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

impl Patterns {
    fn new() -> Option<Self> {
        Some(Self {
            pytest_failed: compile(r"^(?:FAILED|ERROR) (\S+::\S+)")?,
            generic_error: compile(r"(?i)\b(error|failed|failure)\b[:\s](.{0,160})")?,
            cargo_running: compile(r"^Running\s+.*\(([^()]+)\)$")?,
            cargo_doc_tests: compile(r"^Doc-tests\s+(\S+)$")?,
            cargo_test_failed: compile(r"^test (.+) \.\.\. FAILED$")?,
            cargo_binary_died: compile(r"^error: test failed, to rerun pass `")?,
            cargo_build_failed: compile(r"^error: could not compile `([^`]+)`")?,
            cargo_targets_failed: compile(r"^error: \d+ targets? failed:$")?,
            cargo_target_line: compile(r"^`([^`]+)`$")?,
            binary_hash: compile(r"-[0-9a-f]+$")?,
        })
    }
}

/// `target\debug\deps\rho_lexer-0a1b2c3d4e5f6789.exe` -> `rho_lexer`.
fn cargo_binary_name(path: &str, patterns: &Patterns) -> String {
    let stem = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = stem.strip_suffix(".exe").unwrap_or(stem);
    patterns.binary_hash.replace(stem, "").into_owned()
}

/// Binary name for one target in cargo's `N targets failed` summary.
///
/// `-p rho --test courtyard_gpu` -> `courtyard_gpu`, `-p rho-lowering --lib` -> `rho_lowering`.
/// Hyphens become underscores, matching the file names cargo uses under `target/debug/deps`. A
/// target flag (`--test`, `--bin`, `--example`, `--bench`) names the binary directly; `--lib` and
/// `--doc` name it by the crate given with `-p`.
fn cargo_target_binary(target: &str) -> Option<String> {
    let tokens: Vec<&str> = target.split_whitespace().collect();
    let pairs: Vec<(&str, &str)> = tokens.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let flagged = pairs
        .iter()
        .find(|(word, _)| matches!(*word, "--test" | "--bin" | "--example" | "--bench"));
    if let Some((_, name)) = flagged {
        return Some(name.replace('-', "_"));
    }
    let crate_name = pairs
        .iter()
        .rev()
        .find(|(word, _)| *word == "-p")
        .map(|(_, name)| *name);
    if tokens.contains(&"--lib") || tokens.contains(&"--doc") {
        crate_name.map(|name| name.replace('-', "_"))
    } else {
        None
    }
}

/// The state of the cargo scan, folded over the log lines one at a time. Every step returns a
/// new state; nothing is updated in place.
struct CargoState {
    signatures: VectorSync<String>,
    binary: Option<String>,
    failing_binaries: RedBlackTreeSetSync<String>,
    in_summary: bool,
}

impl CargoState {
    fn start() -> Self {
        Self {
            signatures: VectorSync::new_sync(),
            binary: None,
            failing_binaries: RedBlackTreeSetSync::new_sync(),
            in_summary: false,
        }
    }

    fn with_signature(self, signature: String) -> Self {
        Self {
            signatures: self.signatures.push_back(signature),
            ..self
        }
    }

    fn record_failing(self, binary: String) -> Self {
        Self {
            failing_binaries: self.failing_binaries.insert(binary),
            ..self
        }
    }
}

/// A binary that has no failed test of its own is `binary:<name>`, once.
fn note_binary(state: CargoState, name: String) -> CargoState {
    if state.failing_binaries.contains(&name) {
        state
    } else {
        let signature = format!("binary:{name}");
        CargoState {
            signatures: state.signatures.push_back(signature),
            failing_binaries: state.failing_binaries.insert(name),
            ..state
        }
    }
}

/// A failed test is `test:<binary>::<name>`; the binary, when known, also counts as having a
/// failure of its own.
fn test_failed(state: CargoState, test: &str) -> CargoState {
    let owner = state.binary.clone().unwrap_or_default();
    let name = if owner.is_empty() {
        test.to_string()
    } else {
        format!("{owner}::{test}")
    };
    state
        .with_signature(format!("test:{name}"))
        .record_failing(owner)
}

/// One line of the cargo log.
fn cargo_step(state: CargoState, raw: &str, patterns: &Patterns) -> CargoState {
    let line = raw.trim();
    if state.in_summary
        && let Some(caps) = patterns.cargo_target_line.captures(line)
    {
        return match cargo_target_binary(&caps[1]) {
            Some(name) => note_binary(state, name),
            None => state,
        };
    }
    let in_summary = patterns.cargo_targets_failed.is_match(line);
    let state = CargoState {
        in_summary,
        ..state
    };
    if in_summary {
        state
    } else if let Some(caps) = patterns.cargo_running.captures(line) {
        CargoState {
            binary: Some(cargo_binary_name(&caps[1], patterns)),
            ..state
        }
    } else if let Some(caps) = patterns.cargo_doc_tests.captures(line) {
        CargoState {
            binary: Some(caps[1].to_string()),
            ..state
        }
    } else if let Some(caps) = patterns.cargo_test_failed.captures(line) {
        test_failed(state, &caps[1])
    } else if patterns.cargo_binary_died.is_match(line) {
        match state.binary.clone() {
            Some(owner) => note_binary(state, owner),
            None => state,
        }
    } else if let Some(caps) = patterns.cargo_build_failed.captures(line) {
        state.with_signature(format!("build:{}", &caps[1]))
    } else {
        state
    }
}

/// Signatures from `cargo test` output, covering every test binary in the log.
///
/// A failed test is `test:<binary>::<name>` (`test:<name>` when no binary header was seen). A
/// binary that reports `error: test failed` or appears in the `N targets failed` summary without
/// any failed test of its own is `binary:<binary>`. A crate that does not compile is
/// `build:<crate>`.
fn cargo_signatures<S: AsRef<str>>(lines: &[S], patterns: &Patterns) -> Vec<String> {
    let scanned = lines.iter().fold(CargoState::start(), |state, raw| {
        cargo_step(state, raw.as_ref(), patterns)
    });
    let found: Vec<String> = scanned.signatures.iter().cloned().collect();
    dedupe(&found)
}

/// Removes repeated signatures, keeping the first appearance of each.
fn dedupe(signatures: &[String]) -> Vec<String> {
    let (_, kept) = signatures.iter().fold(
        (
            RedBlackTreeSetSync::<String>::new_sync(),
            VectorSync::<String>::new_sync(),
        ),
        |(seen, kept), signature| {
            if seen.contains(signature) {
                (seen, kept)
            } else {
                (
                    seen.insert(signature.clone()),
                    kept.push_back(signature.clone()),
                )
            }
        },
    );
    kept.iter().cloned().collect()
}

/// The last 50 lines are where the pytest and generic parsers look for the failure.
const GENERIC_TAIL_LINES: usize = 50;
/// The generic scan stops after this many signatures.
const GENERIC_MAX_SIGNATURES: usize = 5;
/// A generic signature keeps this many characters of the matched message.
const GENERIC_DETAIL_CHARS: usize = 120;

fn pytest_signatures<S: AsRef<str>>(lines: &[S], patterns: &Patterns) -> Vec<String> {
    lines
        .iter()
        .filter_map(|raw| {
            patterns
                .pytest_failed
                .captures(raw.as_ref().trim())
                .map(|caps| format!("test:{}", &caps[1]))
        })
        .collect()
}

fn generic_signatures<S: AsRef<str>>(lines: &[S], patterns: &Patterns) -> Vec<String> {
    lines
        .iter()
        .skip(lines.len().saturating_sub(GENERIC_TAIL_LINES))
        .filter_map(|raw| {
            patterns.generic_error.captures(raw.as_ref()).map(|caps| {
                let detail = caps.get(2).map_or("", |m| m.as_str());
                let text: String = prefix(detail.trim(), GENERIC_DETAIL_CHARS);
                format!("error:{text}")
            })
        })
        .take(GENERIC_MAX_SIGNATURES)
        .collect()
}

/// Extracts the failure signatures of a check's log lines with the given parser.
///
/// `auto` tries cargo, then pytest, then the generic scan; `cargo` never falls back, so a log
/// without cargo output yields nothing.
pub fn extract_failure_signatures<S: AsRef<str>>(
    lines: &[S],
    parser: FailureParser,
) -> Vec<String> {
    let Some(patterns) = Patterns::new() else {
        return Vec::new();
    };
    if parser == FailureParser::Cargo {
        return cargo_signatures(lines, &patterns);
    }
    let cargo = if parser == FailureParser::Auto {
        cargo_signatures(lines, &patterns)
    } else {
        Vec::new()
    };
    if !cargo.is_empty() {
        return cargo;
    }
    let pytest = if matches!(parser, FailureParser::Auto | FailureParser::Pytest) {
        pytest_signatures(lines, &patterns)
    } else {
        Vec::new()
    };
    let found =
        if pytest.is_empty() && matches!(parser, FailureParser::Auto | FailureParser::Generic) {
            generic_signatures(lines, &patterns)
        } else {
            pytest
        };
    dedupe(&found)
}
