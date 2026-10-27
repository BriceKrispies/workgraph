//! Pure: the git-walking glue of `src/workgraph/experiments/replay.py` and the task shaping of
//! `pool.py` (IF-20, C075, C076): the commit header, the measured candidate, the stub patterns,
//! the replay base's tree entries, the validation verdict, the task body and the views.
//!
//! The shell in [`crate::service::replay`] runs git and the oracle; the counting, the exclusion
//! reasons, the oracle commands and the task text are the core crate `wg_experiments`. This module
//! only assembles them from the text git printed.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value as Json, json};
use wg_clispec::args::{get, items, opt, s, truncate};
use wg_experiments::pool::{self, Entry};
use wg_experiments::replay::{
    self, Change, CommitFacts, Features, NameStatus, ReplayConfig, Split,
};
use wg_json::Value;
use wg_render::Console;
use wg_types::{ErrorCode, WgError};

/// The message of a replay base commit. It must not name the solution commit: a trial agent reads
/// the base's log, and with the SHA it could look the solution up.
pub const BASE_MESSAGE: &str =
    "Replay base\n\nThe tests of the replayed commit, applied to its parent.";

/// The header of a commit: `%P%x00%s%x00%b`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub parents: Vec<String>,
    pub subject: String,
    pub body: String,
}

impl Header {
    /// The first parent, if any.
    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.parents.first().map(String::as_str)
    }
}

/// Parses `git show -s --format=%P%x00%s%x00%b`.
///
/// # Errors
///
/// Returns a `git_error` when the output does not have the three fields.
pub fn parse_header(output: &str) -> Result<Header, WgError> {
    let mut fields = output.splitn(3, '\0');
    match (fields.next(), fields.next(), fields.next()) {
        (Some(parents), Some(subject), Some(body)) => Ok(Header {
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            subject: subject.to_owned(),
            body: body.trim_end_matches('\n').to_owned(),
        }),
        _ => Err(WgError::new(
            ErrorCode::GitError,
            "unexpected commit header from git",
        )),
    }
}

/// A commit analysed as a replay task. `excluded` lists the reasons; empty means eligible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub sha: String,
    pub parent: Option<String>,
    pub subject: String,
    pub body: String,
    pub test_paths: Vec<String>,
    pub source_paths: Vec<String>,
    pub features: Features,
    pub oracle: Vec<Vec<String>>,
    pub excluded: Vec<String>,
}

/// The features as `ReplayFeatures.as_dict`.
#[must_use]
pub fn features_json(features: &Features) -> Json {
    json!({
        "src_lines": features.src_lines,
        "test_lines": features.test_lines,
        "src_files": features.src_files,
        "test_files": features.test_files,
        "packages": features.packages,
        "band": features.band,
    })
}

impl Candidate {
    /// `ReplayCandidate.as_dict`.
    #[must_use]
    pub fn as_json(&self) -> Json {
        json!({
            "sha": self.sha,
            "parent": self.parent,
            "subject": self.subject,
            "body": self.body,
            "test_paths": self.test_paths,
            "source_paths": self.source_paths,
            "features": features_json(&self.features),
            "oracle": self.oracle,
            "excluded": self.excluded,
        })
    }
}

/// The first pass over a commit's `--numstat -z` listing: what changed, split by role, and the
/// features at the commit's tree.
#[derive(Clone, Debug)]
pub struct Measured {
    pub split: Split,
    pub features: Features,
}

/// Measures a listing against the repository's classification and the files of the tree.
#[must_use]
pub fn measure(numstat: &str, files: &BTreeSet<String>, config: &ReplayConfig) -> Measured {
    let split = replay::split_changes(&replay::parse_numstat(numstat), config);
    let features = replay::features(&split, files);
    Measured { split, features }
}

/// Whether the zero-context patch is needed: only a commit with source changes can gain inline
/// tests.
#[must_use]
pub fn needs_patch(measured: &Measured) -> bool {
    !measured.split.sources.is_empty()
}

/// Whether the whitespace-blind listing is needed (a commit with a parent and source lines).
#[must_use]
pub fn needs_whitespace_blind(header: &Header, measured: &Measured) -> bool {
    header.parent().is_some() && measured.features.src_lines > 0
}

/// What the shell read beyond the first pass.
pub struct Extras<'a> {
    /// The zero-context patch, read when [`needs_patch`].
    pub patch: Option<&'a str>,
    /// The whitespace-blind numstat, read when [`needs_whitespace_blind`].
    pub blind_numstat: Option<&'a str>,
}

/// The candidate of a commit: exclusion reasons in the order Python checks them, and the oracle
/// from `resolve_oracle`.
#[must_use]
pub fn candidate(
    sha: &str,
    header: &Header,
    measured: &Measured,
    extras: &Extras<'_>,
    oracle: Vec<Vec<String>>,
    config: &ReplayConfig,
) -> Candidate {
    let sources = &measured.split.sources;
    let inline_tests = !sources.is_empty()
        && extras
            .patch
            .is_some_and(|patch| replay::has_inline_tests(&replay::added_lines(patch), sources));
    let blind_has_source = extras.blind_numstat.is_none_or(|numstat| {
        !replay::split_changes(&replay::parse_numstat(numstat), config)
            .sources
            .is_empty()
    });
    let facts = CommitFacts {
        merge: header.parents.len() > 1,
        root: header.parent().is_none(),
        has_tests: !measured.split.tests.is_empty(),
        src_lines: measured.features.src_lines,
        rename_only: replay::only_pure_renames(sources),
        inline_tests,
        whitespace_blind_has_source: blind_has_source,
        has_oracle: !oracle.is_empty(),
    };
    Candidate {
        sha: sha.to_owned(),
        parent: header.parent().map(str::to_owned),
        subject: header.subject.clone(),
        body: header.body.clone(),
        test_paths: paths(&measured.split.tests),
        source_paths: paths(sources),
        features: measured.features.clone(),
        oracle,
        excluded: replay::exclusion_reasons(&facts)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    }
}

fn paths(changes: &[Change]) -> Vec<String> {
    changes.iter().map(|change| change.path.clone()).collect()
}

/// The test paths of a split, for the oracle.
#[must_use]
pub fn test_paths(measured: &Measured) -> Vec<String> {
    paths(&measured.split.tests)
}

/// The `[package] name` of a `Cargo.toml`, `None` when it does not parse or names no package.
#[must_use]
pub fn package_name(manifest: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(manifest).ok()?;
    let parsed: toml::Table = text.parse().ok()?;
    parsed
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// The directories of the Cargo packages the existing Rust test files belong to: the manifests
/// whose `[package] name` the oracle commands need.
#[must_use]
pub fn package_dirs(test_paths: &[String], files: &BTreeSet<String>) -> Vec<String> {
    let cargo_dirs = replay::manifest_dirs(files, &["Cargo.toml"]);
    test_paths
        .iter()
        .filter(|path| files.contains(path.as_str()) && path.ends_with(".rs"))
        .filter_map(|path| replay::nearest(path, &cargo_dirs))
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The path of the manifest in a package directory (`""` is the root).
#[must_use]
pub fn manifest_path(dir: &str) -> String {
    if dir.is_empty() {
        "Cargo.toml".to_owned()
    } else {
        format!("{dir}/Cargo.toml")
    }
}

/// The mode and blob of the first entry of `ls-tree -z`, `None` when the path is absent.
#[must_use]
pub fn tree_entry(output: &str) -> Option<(String, String)> {
    let entry = output
        .split('\0')
        .next()
        .filter(|entry| !entry.is_empty())?;
    let mut fields = entry.split('\t').next()?.split_whitespace();
    let mode = fields.next()?;
    let _object_type = fields.next()?;
    let blob = fields.next()?;
    Some((mode.to_owned(), blob.to_owned()))
}

/// What to do with one path of the diff between the parent and the commit when building the base:
/// the paths whose index entry is synced to the commit, in order. A renamed-away test file is
/// removed, and a test file at its new path is added.
#[must_use]
pub fn base_sync_paths(entries: &[NameStatus], config: &ReplayConfig) -> Vec<String> {
    entries
        .iter()
        .flat_map(|entry| {
            let renamed_away = match (entry.status.chars().next(), &entry.old_path) {
                (Some('R'), Some(old)) if replay::kind(old, config) == replay::Kind::Test => {
                    Some(old.clone())
                }
                _ => None,
            };
            let at_path = (replay::kind(&entry.path, config) == replay::Kind::Test)
                .then(|| entry.path.clone());
            renamed_away.into_iter().chain(at_path)
        })
        .collect()
}

/// The branch of a replay base: `wg/replay/<sha[:12]>`.
#[must_use]
pub fn base_branch(sha: &str) -> String {
    format!("wg/replay/{}", sha.chars().take(12).collect::<String>())
}

/// The exit codes of the oracle at the solution and at the base, and the verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    pub ok: bool,
    pub reason: Option<String>,
    pub base: Vec<i64>,
    pub solution: Vec<i64>,
}

impl Validation {
    /// A rejection.
    #[must_use]
    pub fn rejected(reason: &str, base: Vec<i64>, solution: Vec<i64>) -> Self {
        Self {
            ok: false,
            reason: Some(reason.to_owned()),
            base,
            solution,
        }
    }
}

/// The verdict of the solution run: red at the solution rejects without a base run.
#[must_use]
pub fn solution_verdict(solution: &[i64]) -> Option<Validation> {
    solution
        .iter()
        .any(|code| *code != 0)
        .then(|| Validation::rejected("oracle_red_at_solution", Vec::new(), solution.to_vec()))
}

/// The verdict once both runs are in: green at the base rejects.
#[must_use]
pub fn final_verdict(base: Vec<i64>, solution: Vec<i64>) -> Validation {
    if base.iter().all(|code| *code == 0) {
        Validation::rejected("oracle_green_at_base", base, solution)
    } else {
        Validation {
            ok: true,
            reason: None,
            base,
            solution,
        }
    }
}

fn stub_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"\btodo!\s*\(",
            r"\bunimplemented!\s*\(",
            r"\bNotImplementedError\b",
            r#"panic!\s*\(\s*"not implemented"#,
        ]
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
    })
}

/// A worktree measured against its base (`WorktreeChanges`).
#[derive(Clone, Debug)]
pub struct WorktreeChanges {
    pub features: Features,
    pub tests_modified: Vec<String>,
    pub stubs: Vec<(String, String)>,
    pub changed_paths: Vec<String>,
}

/// Measures the three listings of a worktree against its base: the numstat, the zero-context
/// patch and the name-status. `files` is the tree of the staged contents.
#[must_use]
pub fn worktree_changes(
    numstat: &str,
    patch: &str,
    name_status: &str,
    files: &BTreeSet<String>,
    config: &ReplayConfig,
) -> WorktreeChanges {
    let measured = measure(numstat, files, config);
    let source_paths: BTreeSet<&str> = measured
        .split
        .sources
        .iter()
        .map(|change| change.path.as_str())
        .collect();
    let stubs = replay::added_lines(patch)
        .into_iter()
        .filter(|(path, _)| source_paths.contains(path.as_str()))
        .filter(|(_, line)| stub_patterns().iter().any(|pattern| pattern.is_match(line)))
        .collect();
    let entries = replay::parse_name_status(name_status);
    WorktreeChanges {
        features: measured.features,
        tests_modified: replay::tests_modified(&entries, config),
        stubs,
        changed_paths: replay::changed_paths(&entries, config),
    }
}

/// One line of a `create` result: `sha`, `status`, `task_key`, `reason`.
#[must_use]
pub fn result_json(sha: &str, status: &str, task_key: Option<&str>, reason: Option<&str>) -> Json {
    json!({"sha": sha, "status": status, "task_key": task_key, "reason": reason})
}

/// `_task_create`: the `TaskCreate` body of a replay task. `validation` carries the exit codes
/// and the time of the validation.
#[must_use]
pub fn task_body(
    candidate: &Candidate,
    repository: &str,
    base_ref: &str,
    validation: Option<(&Validation, &str)>,
) -> Json {
    let features = &candidate.features;
    let entry = Entry {
        sha: &candidate.sha,
        parent: candidate.parent.as_deref(),
        base_ref,
        test_paths: &candidate.test_paths,
        source_paths: &candidate.source_paths,
        oracle: &candidate.oracle,
        features,
        validation: validation
            .map(|(checked, _)| (checked.base.as_slice(), checked.solution.as_slice())),
        validated_at: validation.map(|(_, at)| at),
    };
    json!({
        "title": pool::task_title(&candidate.sha, &candidate.subject),
        "description": pool::task_description(
            &candidate.sha,
            &candidate.subject,
            &candidate.body,
            &candidate.test_paths,
            &candidate.oracle,
            base_ref,
        ),
        "tags": [pool::REPLAY_TAG, pool::POOL_TAG],
        "repositories": [repository],
        "metadata": pool::replay_metadata(&entry),
    })
}

/// The replay tasks of one page of `task list`, as the pool's index wants them: the key, the
/// replayed commit and the classifier (`replay-classifier-1` when the metadata names none).
#[must_use]
pub fn existing_of(page: &Value) -> Vec<(String, Option<String>, String)> {
    items(get(page, "items"))
        .iter()
        .map(|task| {
            let meta = get(task, "metadata").and_then(|m| get(m, "replay"));
            let text = |name: &str| match meta.and_then(|m| get(m, name)) {
                Some(Value::Str(_)) => opt(meta.and_then(|m| get(m, name))),
                _ => None,
            };
            let classifier = match meta.and_then(|m| get(m, "classifier")) {
                None => Some("replay-classifier-1".to_owned()),
                Some(Value::Str(_)) => text("classifier"),
                Some(_) => None,
            };
            (
                s(get(task, "key")),
                text("sha"),
                classifier.unwrap_or_default(),
            )
        })
        .collect()
}

/// The `next_offset` of a page of `task list`.
#[must_use]
pub fn next_offset(page: &Value) -> Option<i64> {
    match get(page, "next_offset") {
        Some(Value::Int(offset)) => i64::try_from(*offset).ok(),
        _ => None,
    }
}

/// The columns of `replay scan`.
const SCAN_HEAD: [&str; 8] = [
    "sha",
    "band",
    "src_lines",
    "test_lines",
    "src_files",
    "packages",
    "excluded",
    "subject",
];

/// The human view of `replay scan`.
#[must_use]
pub fn scan_view(console: &Console, candidates: &Value) -> String {
    let rows: Vec<Vec<String>> = match candidates {
        Value::List(list) => list.iter().map(scan_row).collect(),
        _ => Vec::new(),
    };
    console.table(&rows, &SCAN_HEAD, None).text
}

fn scan_row(candidate: &Value) -> Vec<String> {
    let features = get(candidate, "features");
    let feature = |name: &str| features.and_then(|f| get(f, name));
    let joined =
        |value: Option<&Value>| items(value).iter().map(s_one).collect::<Vec<_>>().join(",");
    vec![
        truncate(&s(get(candidate, "sha")), 12),
        opt(feature("band")).unwrap_or_default(),
        s(feature("src_lines")),
        s(feature("test_lines")),
        s(feature("src_files")),
        joined(feature("packages")),
        joined(get(candidate, "excluded")),
        truncate(&s(get(candidate, "subject")), 60),
    ]
}

fn s_one(value: &Value) -> String {
    s(Some(value))
}

/// The human view of `replay create`.
#[must_use]
pub fn create_view(console: &Console, results: &Value) -> String {
    let rows: Vec<Vec<String>> = match results {
        Value::List(list) => list
            .iter()
            .map(|result| {
                vec![
                    truncate(&s(get(result, "sha")), 12),
                    s(get(result, "status")),
                    opt(get(result, "task_key")).unwrap_or_default(),
                    opt(get(result, "reason")).unwrap_or_default(),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(&rows, &["sha", "status", "task", "reason"], None)
        .text
}

/// `--env K=V`: the pairs, or the `ValidationFailed` of an item without `=` or with an empty key.
///
/// # Errors
///
/// Returns the `ValidationFailed` of a malformed item.
pub fn env_pairs(values: &[String]) -> Result<Vec<(String, String)>, WgError> {
    values
        .iter()
        .map(|item| match item.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
            _ => Err(WgError::new(
                ErrorCode::ValidationFailed,
                format!("--env expects K=V, got {}", wg_model::py_repr_str(item)),
            )),
        })
        .collect()
}

/// `--oracle-template`: a JSON list of strings.
///
/// # Errors
///
/// Returns the `ValidationFailed` of a template that is not a list of strings.
pub fn oracle_template_arg(parsed: &Json) -> Result<Vec<String>, WgError> {
    match parsed {
        Json::Array(list) if list.iter().all(Json::is_string) => Ok(list
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()),
        _ => Err(WgError::new(
            ErrorCode::ValidationFailed,
            "--oracle-template must be a JSON list of strings",
        )),
    }
}

/// A git failure as the `GitError` the Python services raise, with its details.
#[must_use]
pub fn git_failure(error: &wg_gitcore::GitError) -> WgError {
    let failure = WgError::new(ErrorCode::GitError, error.to_string());
    match error {
        wg_gitcore::GitError::Failed {
            args,
            returncode,
            stderr,
        } => failure
            .with_detail("args", json!(args))
            .with_detail("returncode", json!(returncode))
            .with_detail(
                "stderr",
                json!(stderr.chars().take(2000).collect::<String>()),
            ),
        _ => failure,
    }
}

/// `validate_ref`: refuses a ref git could read as an option or that has odd characters.
///
/// # Errors
///
/// Returns the `GitError` Python raises for an invalid ref.
pub fn validate_ref(reference: &str) -> Result<(), WgError> {
    if wg_gitcore::is_valid_ref(reference) {
        Ok(())
    } else {
        Err(WgError::new(
            ErrorCode::GitError,
            format!("invalid git ref {}", wg_model::py_repr_str(reference)),
        ))
    }
}

/// An `int` option as Click reads it: Python's `int()` of the text, else a usage error.
///
/// # Errors
///
/// Returns the usage error of text that is not an integer.
pub fn int_text(
    text: Option<&str>,
    option: &str,
    at: wg_clispec::Usage,
) -> Result<Option<i64>, wg_clispec::CliFailure> {
    text.map(|text| {
        wg_clispec::failure::python_number_text(text)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| at.invalid_value(option, text, "int"))
    })
    .transpose()
}

/// A `float` option as Click reads it: Python's `float()` of the text, else a usage error.
///
/// # Errors
///
/// Returns the usage error of text that is not a number.
pub fn float_text(
    text: Option<&str>,
    option: &str,
    default: f64,
    at: wg_clispec::Usage,
) -> Result<f64, wg_clispec::CliFailure> {
    match text {
        None => Ok(default),
        Some(text) => wg_clispec::failure::python_number_text(text)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| at.invalid_value(option, text, "float")),
    }
}
