//! Replay selection: the pure part of `src/workgraph/experiments/replay.py` (CORE-26, GV-17).
//!
//! A commit that changes source and tests together becomes a replay task. This module classifies
//! the changed files (test, source, ignored, other), reads the numstat and name-status listings,
//! measures the commit's size, decides which commits are excluded and why, and expands the oracle
//! command. Running git, creating the base branch and validating the oracle stay in the shell.
//!
//! Every path is a forward-slash path relative to the repository root. Maps and sets are ordered
//! (`BTreeMap`/`BTreeSet`) so that the output is deterministic.

use std::collections::BTreeSet;
use std::iter::successors;

/// Test globs of a Rust repository (rho's `libraries/*/tests/*.rho` are test programs too).
pub const RUST_TEST_GLOBS: [&str; 3] = ["**/tests/**/*.rs", "**/tests/**/*.rho", "**/*_tests.rs"];
/// Source suffixes of a Rust repository.
pub const RUST_SOURCE_SUFFIXES: [&str; 2] = [".rs", ".rho"];
/// Test globs of a Python repository.
pub const PYTHON_TEST_GLOBS: [&str; 3] = ["tests/**/*.py", "**/test_*.py", "**/*_test.py"];
/// Source suffixes of a Python repository.
pub const PYTHON_SOURCE_SUFFIXES: [&str; 1] = [".py"];
/// Files that are never source or test: lock files and generated output.
pub const IGNORE_GLOBS: [&str; 7] = [
    "Cargo.lock",
    "uv.lock",
    "poetry.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "bun.lock",
    "**/generated/**",
];
/// Manifest names that mark a package root.
pub const MANIFESTS: [&str; 3] = ["Cargo.toml", "pyproject.toml", "package.json"];
/// Bumped whenever file classification changes, so pools built under different rules can be told
/// apart.
pub const CLASSIFIER: &str = "replay-classifier-2";

/// Per-repository classification of changed files and the oracle command template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayConfig {
    /// Globs that mark a test file.
    pub test_globs: Vec<String>,
    /// Suffixes that mark a source file.
    pub source_suffixes: Vec<String>,
    /// Globs that mark an ignored file; they win over the other two.
    pub ignore_globs: Vec<String>,
    /// The oracle command template, when the repository sets one.
    pub oracle_template: Option<Vec<String>>,
}

impl ReplayConfig {
    /// Defaults for a repository with a `Cargo.toml` and/or a `pyproject.toml` at its root.
    #[must_use]
    pub fn for_manifests(has_cargo: bool, has_pyproject: bool) -> Self {
        let globs: Vec<&str> = [
            has_cargo.then_some(RUST_TEST_GLOBS.as_slice()),
            has_pyproject.then_some(PYTHON_TEST_GLOBS.as_slice()),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .copied()
        .collect();
        let suffixes: Vec<&str> = [
            has_cargo.then_some(RUST_SOURCE_SUFFIXES.as_slice()),
            has_pyproject.then_some(PYTHON_SOURCE_SUFFIXES.as_slice()),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .copied()
        .collect();
        Self {
            test_globs: ordered_unique(globs.into_iter().map(str::to_owned)),
            source_suffixes: ordered_unique(suffixes.into_iter().map(str::to_owned)),
            ignore_globs: IGNORE_GLOBS.iter().map(|g| (*g).to_owned()).collect(),
            oracle_template: None,
        }
    }
}

/// What a changed file is for replay selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Lock or generated file: counted nowhere.
    Ignored,
    /// A test file.
    Test,
    /// A source file.
    Source,
    /// Anything else (documentation, configuration).
    Other,
}

impl Kind {
    /// The wire name of the kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Test => "test",
            Self::Source => "source",
            Self::Other => "other",
        }
    }
}

/// Classifies one path: ignored globs first, then test globs, then source suffixes.
#[must_use]
pub fn kind(path: &str, config: &ReplayConfig) -> Kind {
    if config.ignore_globs.iter().any(|g| match_glob(path, g)) {
        Kind::Ignored
    } else if config.test_globs.iter().any(|g| match_glob(path, g)) {
        Kind::Test
    } else if config
        .source_suffixes
        .iter()
        .any(|suffix| path.ends_with(suffix.as_str()))
    {
        Kind::Source
    } else {
        Kind::Other
    }
}

/// Matches a forward-slash path against a root-anchored glob.
///
/// `*` and `?` never cross a `/`; `**` as a whole segment matches any number of segments, zero
/// included, so `**/test_*.py` matches `test_x.py` and `a/b/test_x.py`.
#[must_use]
pub fn match_glob(path: &str, pattern: &str) -> bool {
    let path: Vec<&str> = path.split('/').collect();
    let pattern: Vec<&str> = pattern.split('/').collect();
    match_segments(&path, &pattern)
}

fn match_segments(path: &[&str], pattern: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| match_segments(&path[i..], rest)),
        Some((head, rest)) => path
            .split_first()
            .is_some_and(|(first, tail)| fnmatch_text(first, head) && match_segments(tail, rest)),
    }
}

/// `fnmatch.fnmatchcase` on one path segment.
fn fnmatch_text(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    fnmatch(&name, &pattern)
}

fn fnmatch(name: &[char], pattern: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|i| fnmatch(&name[i..], rest)),
        Some(('?', rest)) => name
            .split_first()
            .is_some_and(|(_, tail)| fnmatch(tail, rest)),
        Some(('[', rest)) => match parse_class(rest) {
            Some((negated, ranges, after)) => name.split_first().is_some_and(|(c, tail)| {
                let inside = ranges.iter().any(|(lo, hi)| lo <= c && c <= hi);
                inside != negated && fnmatch(tail, after)
            }),
            None => name
                .split_first()
                .is_some_and(|(c, tail)| *c == '[' && fnmatch(tail, rest)),
        },
        Some((c, rest)) => name
            .split_first()
            .is_some_and(|(n, tail)| n == c && fnmatch(tail, rest)),
    }
}

type Class<'a> = (bool, Vec<(char, char)>, &'a [char]);

/// Parses the body of a `[...]` class (the text after `[`). A `]` first in the body is literal.
fn parse_class(pattern: &[char]) -> Option<Class<'_>> {
    let (negated, body) = match pattern.first() {
        Some('!' | '^') => (true, &pattern[1..]),
        _ => (false, pattern),
    };
    let close = body.iter().enumerate().skip(1).find(|(_, c)| **c == ']')?.0;
    Some((negated, char_ranges(&body[..close]), &body[close + 1..]))
}

fn char_ranges(members: &[char]) -> Vec<(char, char)> {
    match members {
        [] => Vec::new(),
        [lo, '-', hi, rest @ ..] => [vec![(*lo, *hi)], char_ranges(rest)].concat(),
        [only, rest @ ..] => [vec![(*only, *only)], char_ranges(rest)].concat(),
    }
}

/// The size band of a line count: `floor(log2(lines))`, or `None` for zero and below.
#[must_use]
pub fn band(lines: i64) -> Option<u32> {
    u64::try_from(lines).ok().filter(|n| *n > 0).map(u64::ilog2)
}

/// Whether a commit of `lines` source lines is in the requested range.
#[must_use]
pub fn in_size_range(lines: i64, min_lines: i64, max_lines: Option<i64>) -> bool {
    lines >= min_lines && max_lines.is_none_or(|max| lines <= max)
}

/// One non-binary file in a numstat listing (`path` is the post-image path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Post-image path.
    pub path: String,
    /// Added lines.
    pub added: i64,
    /// Removed lines.
    pub removed: i64,
    /// Pre-image path, for a rename or copy.
    pub old_path: Option<String>,
}

impl Change {
    /// Added plus removed lines.
    #[must_use]
    pub fn lines(&self) -> i64 {
        self.added + self.removed
    }
}

/// One record of `--numstat -z`, and the index of the token after it. A record that carries no
/// usable counts is skipped (`None`); the walk stops when the tokens run out.
fn numstat_step(tokens: &[&str], i: usize) -> Option<(Option<Change>, usize)> {
    let token = tokens.get(i)?.trim_start_matches('\n');
    if token.is_empty() {
        return Some((None, i + 1));
    }
    let fields: Vec<&str> = token.splitn(3, '\t').collect();
    let (added, removed, path) = match fields.as_slice() {
        [added, removed, path] => (*added, *removed, *path),
        _ => return Some((None, i + 1)),
    };
    let (old_path, path, next) = if path.is_empty() {
        (
            Some((*tokens.get(i + 1)?).to_owned()),
            (*tokens.get(i + 2)?).to_owned(),
            i + 3,
        )
    } else {
        (None, path.to_owned(), i + 1)
    };
    if added == "-" || removed == "-" {
        return Some((None, next));
    }
    match (added.parse::<i64>(), removed.parse::<i64>()) {
        (Ok(added), Ok(removed)) => Some((
            Some(Change {
                path,
                added,
                removed,
                old_path,
            }),
            next,
        )),
        _ => Some((None, next)),
    }
}

/// Parses `--numstat -z`: `added TAB removed TAB path NUL`, or for renames `... TAB NUL old NUL new
/// NUL`. Binary files (`-` counts) carry no line counts and are left out.
#[must_use]
pub fn parse_numstat(output: &str) -> Vec<Change> {
    let tokens: Vec<&str> = output.split('\0').collect();
    successors(numstat_step(&tokens, 0), |(_, next)| {
        numstat_step(&tokens, *next)
    })
    .filter_map(|(change, _)| change)
    .collect()
}

/// One record of `--name-status -z`. `path` is the post-image path; renames and copies carry the
/// pre-image path in `old_path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameStatus {
    /// Status letter, with a similarity score for renames and copies (`R85`).
    pub status: String,
    /// Post-image path.
    pub path: String,
    /// Pre-image path, for a rename or copy.
    pub old_path: Option<String>,
}

fn name_status_step(tokens: &[&str], i: usize) -> Option<(Option<NameStatus>, usize)> {
    let status = *tokens.get(i)?;
    if status.is_empty() {
        return Some((None, i + 1));
    }
    if matches!(status.chars().next(), Some('R' | 'C')) {
        let old = *tokens.get(i + 1)?;
        let new = *tokens.get(i + 2)?;
        Some((
            Some(NameStatus {
                status: status.to_owned(),
                path: new.to_owned(),
                old_path: Some(old.to_owned()),
            }),
            i + 3,
        ))
    } else {
        let path = *tokens.get(i + 1)?;
        Some((
            Some(NameStatus {
                status: status.to_owned(),
                path: path.to_owned(),
                old_path: None,
            }),
            i + 2,
        ))
    }
}

/// Parses `--name-status -z`.
#[must_use]
pub fn parse_name_status(output: &str) -> Vec<NameStatus> {
    let tokens: Vec<&str> = output.split('\0').collect();
    successors(name_status_step(&tokens, 0), |(_, next)| {
        name_status_step(&tokens, *next)
    })
    .filter_map(|(entry, _)| entry)
    .collect()
}

/// Added line contents (without the `+`) per post-image path, from a zero-context patch, in the
/// order they appear.
#[must_use]
pub fn added_lines(patch: &str) -> Vec<(String, String)> {
    patch
        .split('\n')
        .fold(PatchState::default(), patch_step)
        .added
        .iter()
        .cloned()
        .collect()
}

/// The state of the patch walk. `added` is persistent so that each step shares the prefix.
#[derive(Clone, Debug, Default)]
struct PatchState {
    current: Option<String>,
    in_hunk: bool,
    added: rpds::VectorSync<(String, String)>,
}

fn patch_step(state: PatchState, line: &str) -> PatchState {
    if line.starts_with("diff --git ") {
        PatchState {
            current: None,
            in_hunk: false,
            added: state.added,
        }
    } else if line.starts_with("@@") {
        PatchState {
            in_hunk: true,
            ..state
        }
    } else if state.in_hunk {
        match (&state.current, line.strip_prefix('+')) {
            (Some(path), Some(content)) => PatchState {
                added: state.added.push_back((path.clone(), content.to_owned())),
                ..state
            },
            _ => state,
        }
    } else if let Some(path) = line.strip_prefix("+++ b/") {
        PatchState {
            current: Some(path.to_owned()),
            ..state
        }
    } else {
        state
    }
}

/// Whether a source file gained an inline test: a Rust `#[test]` or `#[cfg(test)]`, or a Python
/// `def test_` line.
#[must_use]
pub fn has_inline_tests(added: &[(String, String)], sources: &[Change]) -> bool {
    sources.iter().any(|change| {
        let is_python = change.path.ends_with(".py");
        added.iter().any(|(path, line)| {
            *path == change.path
                && (line.contains("#[test]")
                    || line.contains("#[cfg(test)]")
                    || (is_python && line.trim_start().starts_with("def test_")))
        })
    })
}

/// Splits a path into its directory and final segment, as Python's `rpartition("/")` does: the
/// directory is empty for a file at the root.
fn split_dir(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// The directories that hold a file named one of `names`.
#[must_use]
pub fn manifest_dirs(files: &BTreeSet<String>, names: &[&str]) -> BTreeSet<String> {
    files
        .iter()
        .filter(|path| names.contains(&split_dir(path).1))
        .map(|path| split_dir(path).0.to_owned())
        .collect()
}

/// The nearest directory at or above the file's directory that is in `dirs` (`""` is the root).
#[must_use]
pub fn nearest(path: &str, dirs: &BTreeSet<String>) -> Option<String> {
    nearest_from(split_dir(path).0, dirs)
}

fn nearest_from(current: &str, dirs: &BTreeSet<String>) -> Option<String> {
    if dirs.contains(current) {
        Some(current.to_owned())
    } else if current.is_empty() {
        None
    } else {
        nearest_from(split_dir(current).0, dirs)
    }
}

/// Expands an oracle template: `{test_files}` becomes the test paths, `{packages}` the packages.
#[must_use]
pub fn expand_oracle_template(
    template: &[String],
    test_paths: &[String],
    packages: &[String],
) -> Vec<String> {
    template
        .iter()
        .flat_map(|item| match item.as_str() {
            "{test_files}" => test_paths.to_vec(),
            "{packages}" => packages.to_vec(),
            other => vec![other.to_owned()],
        })
        .collect()
}

/// The test paths present in `files`, and the packages they belong to (the nearest manifest
/// directory, `.` for a file under the root manifest).
#[must_use]
pub fn template_inputs(
    files: &BTreeSet<String>,
    test_paths: &[String],
) -> (Vec<String>, Vec<String>) {
    let existing: Vec<String> = test_paths
        .iter()
        .filter(|path| files.contains(path.as_str()))
        .cloned()
        .collect();
    let manifests = manifest_dirs(files, &MANIFESTS);
    let packages = ordered_unique(existing.iter().map(|path| match nearest(path, &manifests) {
        Some(dir) if !dir.is_empty() => dir,
        _ => ".".to_owned(),
    }));
    (existing, packages)
}

/// The oracle commands for test files at a commit, from the commit's own files.
///
/// `package_name` gives the `[package] name` of the Cargo package at a directory, read by the
/// shell. A repository with an oracle template expands it instead.
#[must_use]
pub fn resolve_oracle(
    config: &ReplayConfig,
    test_paths: &[String],
    files: &BTreeSet<String>,
    package_name: impl Fn(&str) -> Option<String>,
) -> Vec<Vec<String>> {
    if let Some(template) = &config.oracle_template {
        let (existing, packages) = template_inputs(files, test_paths);
        return if existing.is_empty() {
            Vec::new()
        } else {
            vec![expand_oracle_template(template, &existing, &packages)]
        };
    }
    let existing: Vec<String> = test_paths
        .iter()
        .filter(|path| files.contains(path.as_str()))
        .cloned()
        .collect();
    let cargo_dirs = manifest_dirs(files, &["Cargo.toml"]);
    let rust_commands = existing
        .iter()
        .filter(|path| path.ends_with(".rs"))
        .filter_map(|path| {
            let package_dir = nearest(path, &cargo_dirs)?;
            let name = package_name(&package_dir)?;
            let rel = if package_dir.is_empty() {
                path.as_str()
            } else {
                &path[package_dir.len() + 1..]
            };
            if rel.starts_with("tests/") && rel.matches('/').count() == 1 {
                let target = &rel["tests/".len()..rel.len() - ".rs".len()];
                Some(argv(&["cargo", "test", "-p", &name, "--test", target]))
            } else if rel.starts_with("src/") && path.ends_with("_tests.rs") {
                Some(argv(&["cargo", "test", "-p", &name, "--lib"]))
            } else {
                None
            }
        });
    let python_tests: Vec<String> = existing
        .iter()
        .filter(|path| path.ends_with(".py"))
        .cloned()
        .collect();
    let python_command = (!python_tests.is_empty()).then(|| {
        [
            vec![
                "python".to_owned(),
                "-m".to_owned(),
                "pytest".to_owned(),
                "-q".to_owned(),
            ],
            python_tests,
        ]
        .concat()
    });
    let commands: Vec<Vec<String>> = rust_commands.chain(python_command).collect();
    ordered_unique(commands)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// The changes split by role, in the order the listing gave them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    /// Source changes.
    pub sources: Vec<Change>,
    /// Test changes.
    pub tests: Vec<Change>,
}

/// Splits changes into source and test changes.
#[must_use]
pub fn split_changes(changes: &[Change], config: &ReplayConfig) -> Split {
    Split {
        sources: changes
            .iter()
            .filter(|c| kind(&c.path, config) == Kind::Source)
            .cloned()
            .collect(),
        tests: changes
            .iter()
            .filter(|c| kind(&c.path, config) == Kind::Test)
            .cloned()
            .collect(),
    }
}

/// Size and shape of a commit's change. `src_lines` is the primary, log2-banded size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    /// Added plus removed lines of source files.
    pub src_lines: i64,
    /// Added plus removed lines of test files.
    pub test_lines: i64,
    /// Number of source files.
    pub src_files: usize,
    /// Number of test files.
    pub test_files: usize,
    /// Sorted package roots of the source files (`""` is the repository root).
    pub packages: Vec<String>,
    /// `band(src_lines)`.
    pub band: Option<u32>,
}

/// The features of a split change, with packages read from the files at the commit.
#[must_use]
pub fn features(split: &Split, files: &BTreeSet<String>) -> Features {
    let manifests = manifest_dirs(files, &MANIFESTS);
    let src_lines: i64 = split.sources.iter().map(Change::lines).sum();
    let packages: BTreeSet<String> = split
        .sources
        .iter()
        .map(|change| nearest(&change.path, &manifests).unwrap_or_default())
        .collect();
    Features {
        src_lines,
        test_lines: split.tests.iter().map(Change::lines).sum(),
        src_files: split.sources.len(),
        test_files: split.tests.len(),
        packages: packages.into_iter().collect(),
        band: band(src_lines),
    }
}

/// Existing test files that were modified, deleted, or renamed away. New test files are not
/// listed.
#[must_use]
pub fn tests_modified(entries: &[NameStatus], config: &ReplayConfig) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| {
            let letter = entry.status.chars().next();
            match (letter, &entry.old_path) {
                (Some('R'), Some(old)) if kind(old, config) == Kind::Test => Some(old.clone()),
                (Some('M' | 'D'), _) if kind(&entry.path, config) == Kind::Test => {
                    Some(entry.path.clone())
                }
                _ => None,
            }
        })
        .collect()
}

/// Every source or test path that differs from the base, including the old name of a rename,
/// sorted.
#[must_use]
pub fn changed_paths(entries: &[NameStatus], config: &ReplayConfig) -> Vec<String> {
    entries
        .iter()
        .flat_map(|entry| [Some(entry.path.as_str()), entry.old_path.as_deref()])
        .flatten()
        .filter(|path| matches!(kind(path, config), Kind::Source | Kind::Test))
        .map(str::to_owned)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Whether a commit's sources are all pure renames: at least one source, and every source has an
/// old path and no changed lines.
#[must_use]
pub fn only_pure_renames(sources: &[Change]) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|change| change.old_path.is_some() && change.lines() == 0)
}

/// The facts a commit is excluded on, as the shell measures them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFacts {
    /// The commit has more than one parent.
    pub merge: bool,
    /// The commit has no parent.
    pub root: bool,
    /// The commit changes at least one test file.
    pub has_tests: bool,
    /// Source lines changed.
    pub src_lines: i64,
    /// The source changes are all pure renames ([`only_pure_renames`]).
    pub rename_only: bool,
    /// A source file gained an inline test ([`has_inline_tests`]).
    pub inline_tests: bool,
    /// The change with whitespace ignored still touches a source file.
    pub whitespace_blind_has_source: bool,
    /// The commit yields an oracle command.
    pub has_oracle: bool,
}

/// The exclusion reasons of a commit, in the order the Python source checks them. Empty means the
/// commit is eligible.
#[must_use]
pub fn exclusion_reasons(facts: &CommitFacts) -> Vec<&'static str> {
    [
        (facts.merge, "merge"),
        (facts.root, "root"),
        (!facts.has_tests, "no_tests"),
        (facts.src_lines == 0, "no_source"),
        (facts.inline_tests, "inline_tests"),
        (
            !facts.root && facts.src_lines > 0 && !facts.whitespace_blind_has_source,
            "formatting_only",
        ),
        (facts.rename_only, "rename_only"),
        (!facts.has_oracle, "no_oracle"),
    ]
    .into_iter()
    .filter(|(excluded, _)| *excluded)
    .map(|(_, reason)| reason)
    .collect()
}

/// Keeps the first occurrence of each item.
fn ordered_unique<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    items.into_iter().fold(Vec::new(), |acc, item| {
        if acc.contains(&item) {
            acc
        } else {
            acc.into_iter().chain(std::iter::once(item)).collect()
        }
    })
}
