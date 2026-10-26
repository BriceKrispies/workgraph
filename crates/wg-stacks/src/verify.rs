//! Verification on the integration tip: the judgement of the tip run, the base outcomes it is
//! compared with (reused from an earlier run at the base when one ran the same check definition),
//! errored checks, the pass that verifies the merged items, and the job key that collapses repeated
//! requests. Port of the decision steps of `StackVerifier.verify`, `_verification_errored`,
//! `_passed`, `_review_parent`, `_base_outcomes`, `_cached_outcomes`, `_enqueue`, `_in_scope`,
//! `_at_bases`, `_ran_with` and `_tail` in `integrations/git/stack_verification.py`.
//!
//! The shell runs the checks, reads earlier runs from the store and writes events and transitions;
//! every choice it makes along the way is one of these functions.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use wg_types::{StackItemState, TaskStatus, VerificationStatus};
use wg_verify::{Baseline, CheckSpec, CheckTier};

use crate::ordered::{dict_from, sorted};
use crate::regression::{
    BaseOutcomes, CheckOutcome, Regressions, RunCheck, RunResult, errored_checks, failed_checks,
    regressions,
};

/// How many of the workspace's most recent verification runs are scanned for a reusable base
/// outcome (`_CACHE_SCAN_LIMIT`).
pub const CACHE_SCAN_LIMIT: usize = 500;

/// How many trailing output lines an errored-check report keeps.
pub const ERROR_TAIL_LINES: usize = 20;

/// The `stack.verified` reason when every full check passed on the tip.
pub const REASON_FULL_PASSED: &str = "full checks passed";
/// The `stack.verified` reason when every failure is already on the base.
pub const REASON_NO_NEW_FAILURES: &str = "no new failures against the base";
/// The `stack.verified` reason when the stack has no full checks.
pub const REASON_NO_CHECKS: &str = "no full checks are configured";

/// One repository of a stack with the commit the stack was cut from (`StackRepoBase`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRepoBase {
    pub repository_id: String,
    pub name: String,
    pub base_sha: String,
}

/// Repository name to base commit, for every repository of the stack.
pub fn bases(repos: &[StackRepoBase]) -> BTreeMap<String, String> {
    repos
        .iter()
        .map(|repo| (repo.name.clone(), repo.base_sha.clone()))
        .collect()
}

/// `_in_scope`: a full check that is workspace- or project-wide, or that names one of the stack's
/// repositories by name or id.
pub fn in_scope(repos: &[StackRepoBase], spec: &CheckSpec) -> bool {
    spec.tier == CheckTier::Full
        && spec.repository.as_ref().is_none_or(|wanted| {
            repos
                .iter()
                .any(|repo| repo.name == *wanted || repo.repository_id == *wanted)
        })
}

/// Check name to baseline mode, for the specs of the checks in scope.
pub fn baselines(specs: &[CheckSpec]) -> BTreeMap<String, Baseline> {
    specs
        .iter()
        .map(|spec| (spec.name.clone(), spec.baseline))
        .collect()
}

/// Where a verification stopped on a check that could not judge its revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Tip,
    Base,
    Bisect,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tip => "tip",
            Self::Base => "base",
            Self::Bisect => "bisect",
        }
    }
}

/// The last `lines` lines of a check's output (`_tail`), cut the way Python's `splitlines` cuts.
pub fn tail(excerpt: &str, lines: usize) -> String {
    let all = split_lines(excerpt);
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// `str.splitlines()`: lines end at `\r\n`, `\n`, `\r` and the other Unicode line boundaries.
fn split_lines(text: &str) -> Vec<&str> {
    // (start, end) of each line break; `\r\n` is one break, its `\n` is not another.
    let breaks: Vec<(usize, usize)> = text
        .char_indices()
        .filter(|(at, c)| is_line_break(*c) && !(*c == '\n' && text[..*at].ends_with('\r')))
        .map(|(at, c)| {
            let crlf = c == '\r' && text[at + 1..].starts_with('\n');
            (at, at + c.len_utf8() + usize::from(crlf))
        })
        .collect();
    let starts = std::iter::once(0).chain(breaks.iter().map(|(_, end)| *end));
    let last = breaks.last().map_or(0, |(_, end)| *end);
    breaks
        .iter()
        .zip(starts)
        .map(|((at, _), start)| &text[start..*at])
        .chain((last < text.len()).then(|| &text[last..]))
        .collect()
}

fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Errored checks with the tails of their output, by name.
pub fn error_tails(checks: &[&RunCheck]) -> Vec<(String, String)> {
    dict_from(checks.iter().map(|check| {
        (
            check.name.clone(),
            tail(&check.output_excerpt, ERROR_TAIL_LINES),
        )
    }))
}

/// A verification stopped on checks that could not judge their revision: nothing passes, nothing
/// is bisected or ejected, and the stack returns to `integrating`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Errored {
    pub stage: Stage,
    /// The run whose check errored; `None` only for a base outcome without a run.
    pub run_id: Option<String>,
    /// Check name to the tail of its output.
    pub errors: Vec<(String, String)>,
}

impl Errored {
    /// The errored check names, sorted.
    pub fn checks(&self) -> Vec<String> {
        sorted(self.errors.iter().map(|(name, _)| name.clone()))
    }

    /// The reason recorded on the event and the stack's status change.
    pub fn reason(&self) -> String {
        format!(
            "verification could not judge {} (run {}, {})",
            self.checks().join(", "),
            self.run_id.as_deref().unwrap_or("None"),
            self.stage.as_str()
        )
    }

    /// The `stack.verification_errored` event data.
    pub fn event_data(&self) -> Value {
        let excerpt: Map<String, Value> = self
            .errors
            .iter()
            .map(|(name, text)| (name.clone(), Value::String(text.clone())))
            .collect();
        json!({
            "run_id": self.run_id,
            "stage": self.stage.as_str(),
            "checks": self.checks(),
            "excerpt": excerpt,
            "reason": self.reason(),
        })
    }
}

/// What the tip run decides before any base outcome is gathered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TipStep {
    /// A required check errored on the tip.
    Errored(Errored),
    /// Every required check passed: the stack verifies with [`REASON_FULL_PASSED`].
    Passed,
    /// Gather the base outcomes of these failed checks (name to the repository the tip result was
    /// recorded against), then call [`judge_against_base`].
    MeasureBase(Vec<(String, Option<String>)>),
}

/// The first decision on the tip run.
pub fn judge_tip(run: &RunResult) -> TipStep {
    let errored = errored_checks(&run.checks);
    if !errored.is_empty() {
        return TipStep::Errored(Errored {
            stage: Stage::Tip,
            run_id: Some(run.id.clone()),
            errors: error_tails(&errored),
        });
    }
    if run.required_passed {
        return TipStep::Passed;
    }
    TipStep::MeasureBase(failed_checks(&run.checks))
}

/// The decision once the base outcomes are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseVerdict {
    /// A failed check errored on the base: stop with stage `base`.
    Errored(Errored),
    /// Every failure is already on the base: the stack verifies with [`REASON_NO_NEW_FAILURES`],
    /// recording the tolerated signatures.
    Tolerated(Vec<(String, Vec<String>)>),
    /// New failures: bisect the merge sequence.
    Regressed(Regressions),
}

/// Judges the failing tip run against the base outcomes of its failed checks.
pub fn judge_against_base(
    run: &RunResult,
    base: &BaseOutcomes,
    baselines: &BTreeMap<String, Baseline>,
) -> BaseVerdict {
    let base_errors: Vec<(&str, &CheckOutcome)> = base
        .iter()
        .filter(|(_, outcome)| outcome.status == VerificationStatus::Error)
        .collect();
    if let Some((_, first)) = base_errors.first() {
        let errors =
            dict_from(base_errors.iter().map(|(name, outcome)| {
                (name.to_string(), tail(&outcome.excerpt, ERROR_TAIL_LINES))
            }));
        return BaseVerdict::Errored(Errored {
            stage: Stage::Base,
            run_id: first.run_id.clone(),
            errors,
        });
    }
    let found = regressions(&run.checks, Some(base), baselines);
    if found.is_empty() {
        BaseVerdict::Tolerated(tolerated(run))
    } else {
        BaseVerdict::Regressed(found)
    }
}

/// The signatures of every check whose name failed, as `stack.verified` records them.
pub fn tolerated(run: &RunResult) -> Vec<(String, Vec<String>)> {
    let failed = failed_checks(&run.checks);
    dict_from(
        run.checks
            .iter()
            .filter(|check| failed.iter().any(|(name, _)| *name == check.name))
            .map(|check| (check.name.clone(), check.failure_signatures.clone())),
    )
}

/// The whole judgement of a tip run whose base outcomes are already known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyDecision {
    Errored(Errored),
    Passed {
        reason: &'static str,
        /// Present when failures were tolerated against the base.
        tolerated: Option<Vec<(String, Vec<String>)>>,
    },
    Bisect(Regressions),
}

/// `verify` from its errored-check test to the bisect branch.
pub fn verify_decision(
    run: &RunResult,
    base: &BaseOutcomes,
    baselines: &BTreeMap<String, Baseline>,
) -> VerifyDecision {
    match judge_tip(run) {
        TipStep::Errored(errored) => VerifyDecision::Errored(errored),
        TipStep::Passed => VerifyDecision::Passed {
            reason: REASON_FULL_PASSED,
            tolerated: None,
        },
        TipStep::MeasureBase(_) => match judge_against_base(run, base, baselines) {
            BaseVerdict::Errored(errored) => VerifyDecision::Errored(errored),
            BaseVerdict::Tolerated(found) => VerifyDecision::Passed {
                reason: REASON_NO_NEW_FAILURES,
                tolerated: Some(found),
            },
            BaseVerdict::Regressed(found) => VerifyDecision::Bisect(found),
        },
    }
}

// ---------------------------------------------------------------- base outcomes

/// `_at_bases`: whether a revision manifest's entries hold exactly the base commit of every stack
/// repository. A later entry for the same repository wins, as in the Python dictionary.
pub fn at_bases(entries: &[Value], repos: &[StackRepoBase]) -> bool {
    repos.iter().all(|repo| {
        entries
            .iter()
            .rev()
            .find(|entry| {
                entry.get("repository_id").and_then(Value::as_str) == Some(&repo.repository_id)
            })
            .and_then(|entry| entry.get("sha"))
            .and_then(Value::as_str)
            == Some(repo.base_sha.as_str())
    })
}

/// `_ran_with`: whether a run's config snapshot holds the check `name` with exactly the current
/// definition. The snapshot entry's `config_id` and `config` keys are not part of the definition.
pub fn ran_with(config_snapshot: &[Value], name: &str, spec: &CheckSpec) -> bool {
    let current = Value::Object(spec.to_json());
    config_snapshot
        .iter()
        .filter(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .filter_map(Value::as_object)
        .any(|entry| {
            let definition: Map<String, Value> = entry
                .iter()
                .filter(|(key, _)| *key != "config_id" && *key != "config")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            py_equal(&Value::Object(definition), &current)
        })
}

/// JSON equality as Python compares the decoded values: numbers by value (`900 == 900.0`,
/// `True == 1`), objects regardless of key order.
fn py_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| py_equal(value, other)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_equal(p, q))
        }
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// An earlier verification run of the workspace, as the base cache reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedRun {
    pub id: String,
    /// The entries of the run's revision manifest; `None` when the run has no manifest.
    pub manifest_entries: Option<Vec<Value>>,
    pub config_snapshot: Vec<Value>,
    /// The run's check results.
    pub checks: Vec<RunCheck>,
}

/// The check names the cache still has to find: failed, not found yet, and with a current spec.
pub fn pending_checks(
    failed: &[(String, Option<String>)],
    found: &BaseOutcomes,
    specs: &BTreeMap<String, CheckSpec>,
) -> Vec<String> {
    failed
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !found.contains(name) && specs.contains_key(*name))
        .cloned()
        .collect()
}

/// `_cached_outcomes`: base outcomes of the failed checks from earlier runs whose manifest is at
/// the bases. `runs` are the newest [`CACHE_SCAN_LIMIT`] runs of the workspace, newest first.
///
/// A result is taken only when its check name and repository match the tip result, it did not
/// error, and its run ran the check with the current definition.
pub fn cached_outcomes(
    runs: &[CachedRun],
    repos: &[StackRepoBase],
    failed: &[(String, Option<String>)],
    specs: &BTreeMap<String, CheckSpec>,
) -> BaseOutcomes {
    // Once every failed check is found, later runs add nothing (the Python loop stops there).
    runs.iter().fold(BaseOutcomes::new(), |found, run| {
        let pending = pending_checks(failed, &found, specs);
        let at_base = run
            .manifest_entries
            .as_deref()
            .is_some_and(|entries| at_bases(entries, repos));
        if pending.is_empty() || !at_base {
            return found;
        }
        run.checks
            .iter()
            .filter(|check| pending.contains(&check.name))
            .fold(found, |found, check| {
                let repository = failed
                    .iter()
                    .find(|(name, _)| *name == check.name)
                    .map(|(_, repository)| repository);
                let reusable = !found.contains(&check.name)
                    && repository == Some(&check.repository_id)
                    && check.status != VerificationStatus::Error
                    && specs
                        .get(&check.name)
                        .is_some_and(|spec| ran_with(&run.config_snapshot, &check.name, spec));
                if reusable {
                    found.with(check.name.clone(), outcome_of(check, &run.id))
                } else {
                    found
                }
            })
    })
}

/// The failed checks the cache did not answer, which a new run at the bases must measure; sorted.
pub fn missing_base_checks(
    failed: &[(String, Option<String>)],
    cached: &BaseOutcomes,
) -> Vec<String> {
    failed
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| !cached.contains(name))
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The base outcomes with the results of a new run at the bases added: each missing check whose
/// result was recorded against the same repository as on the tip. An errored result is kept, and
/// the caller stops on it.
pub fn with_base_run(
    outcomes: BaseOutcomes,
    run_id: &str,
    checks: &[RunCheck],
    missing: &[String],
    failed: &[(String, Option<String>)],
) -> BaseOutcomes {
    checks
        .iter()
        .filter(|check| missing.contains(&check.name))
        .filter(|check| {
            failed
                .iter()
                .find(|(name, _)| *name == check.name)
                .is_some_and(|(_, repository)| *repository == check.repository_id)
        })
        .fold(outcomes, |outcomes, check| {
            outcomes.with(check.name.clone(), outcome_of(check, run_id))
        })
}

fn outcome_of(check: &RunCheck, run_id: &str) -> CheckOutcome {
    CheckOutcome {
        status: check.status,
        signatures: check.failure_signatures.clone(),
        run_id: Some(run_id.to_string()),
        excerpt: check.output_excerpt.clone(),
    }
}

// ---------------------------------------------------------------- pass

/// One stack item as the pass reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassedRow {
    pub task_key: String,
    pub state: StackItemState,
    pub task_status: TaskStatus,
}

/// What a passing verification changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassedPlan {
    /// Merged items that become `verified`, by task key, in queue order.
    pub verified: Vec<String>,
    /// Tasks of verified items that move from `awaiting_verification` to `awaiting_review`.
    pub to_review: Vec<String>,
}

/// `_passed`: every merged item verifies, and every verified child awaiting verification becomes
/// reviewable. `rows` are in queue order.
pub fn passed_plan(rows: &[PassedRow]) -> PassedPlan {
    let verified: Vec<String> = rows
        .iter()
        .filter(|row| row.state == StackItemState::Merged)
        .map(|row| row.task_key.clone())
        .collect();
    let to_review = rows
        .iter()
        .filter(|row| {
            matches!(row.state, StackItemState::Merged | StackItemState::Verified)
                && row.task_status == TaskStatus::AwaitingVerification
        })
        .map(|row| row.task_key.clone())
        .collect();
    PassedPlan {
        verified,
        to_review,
    }
}

/// The reason a child or the parent is moved to review after the stack verified.
pub fn verified_reason(parent_key: &str, run_id: Option<&str>) -> String {
    match run_id {
        Some(run) => {
            format!("stack {parent_key} verified on the integration tip (verification run {run})")
        }
        None => format!("stack {parent_key} verified on the integration tip"),
    }
}

/// `_review_parent`: the lifecycle steps that walk the parent to `awaiting_review`. Empty for a
/// parent already there and for a status with no path, which [`parent_reviewable`] tells apart.
pub fn review_parent_steps(status: TaskStatus) -> &'static [TaskStatus] {
    use TaskStatus as T;
    match status {
        T::AwaitingVerification | T::Claimed | T::Running => &[T::AwaitingReview],
        T::Ready => &[T::Claimed, T::AwaitingReview],
        T::Backlog | T::Blocked | T::Failed => &[T::Ready, T::Claimed, T::AwaitingReview],
        T::AwaitingReview | T::Accepted | T::Cancelled => &[],
    }
}

/// Whether the parent can be walked to `awaiting_review` (or is there already).
pub fn parent_reviewable(status: TaskStatus) -> bool {
    status == TaskStatus::AwaitingReview || !review_parent_steps(status).is_empty()
}

/// The message recorded when the parent cannot be reviewed yet.
pub fn not_reviewable_message(parent_key: &str, status: TaskStatus) -> String {
    format!(
        "{parent_key} is {} and cannot be reviewed yet",
        status.as_str()
    )
}

/// The `stack.verified` event data.
pub fn verified_event(
    run_id: Option<&str>,
    tips: &BTreeMap<String, String>,
    tasks: &[String],
    reason: &str,
    tolerated: Option<&[(String, Vec<String>)]>,
) -> Value {
    let fields = [
        ("run_id", json!(run_id)),
        ("tips", json!(tips)),
        ("tasks", json!(tasks)),
        ("reason", json!(reason)),
    ];
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .chain(tolerated.map(|found| ("tolerated".to_string(), signature_map(found))))
            .collect(),
    )
}

/// Check name to signatures as a JSON object, in the given order.
pub fn signature_map(found: &[(String, Vec<String>)]) -> Value {
    Value::Object(
        found
            .iter()
            .map(|(name, signatures)| (name.clone(), json!(signatures)))
            .collect(),
    )
}

/// The `stack.verification_failed` event data.
pub fn failed_event(run_id: &str, tips: &BTreeMap<String, String>, failing: &Regressions) -> Value {
    json!({
        "run_id": run_id,
        "tips": tips,
        "failed_checks": crate::regression::sorted_names(failing),
        "signatures": signature_map(failing),
    })
}

/// The `stack.bisect_probe` event data.
pub fn probe_event(
    position: usize,
    shas: &BTreeMap<String, String>,
    passed: bool,
    run_id: &str,
    failed_checks: &[String],
) -> Value {
    json!({
        "position": position,
        "shas": shas,
        "passed": passed,
        "run_id": run_id,
        "failed_checks": failed_checks,
    })
}

// ---------------------------------------------------------------- jobs

/// The idempotency key of a `stack_verify` job: repeated requests for the same integration tips
/// collapse; an on-demand request adds the stack version. `tips` pairs repository names with tips
/// in repository-name order.
pub fn verify_job_key(stack_id: &str, tips: &[(String, String)], version: Option<&str>) -> String {
    let joined = tips
        .iter()
        .map(|(name, tip)| format!("{name}={tip}"))
        .collect::<Vec<_>>()
        .join(",");
    match version.filter(|v| !v.is_empty()) {
        Some(version) => format!("stack-verify:{stack_id}:{joined}:{version}"),
        None => format!("stack-verify:{stack_id}:{joined}"),
    }
}

/// The resource keys a `stack_verify` job holds: the stack's exclusive key and the verification
/// key of each of its repositories, sorted and without repeats.
pub fn verify_resource_keys(stack_id: &str, repository_ids: &[String]) -> Vec<String> {
    repository_ids
        .iter()
        .flat_map(|id| {
            [
                format!("exclusive:stack:{stack_id}:{id}"),
                format!("verify:{id}"),
            ]
        })
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Whether a job is a queued or running verification of `stack_id`, which a manual ejection
/// cancels.
pub fn is_cancellable_verify_job(
    kind: &str,
    status: &str,
    payload: &Value,
    stack_id: &str,
) -> bool {
    kind == "stack_verify"
        && matches!(status, "queued" | "running")
        && payload.get("stack_id").and_then(Value::as_str) == Some(stack_id)
}

/// The integration tip of a repository, or its base while the worktree does not exist yet.
pub fn tip_or_base(tip: Option<&str>, base_sha: &str) -> String {
    tip.filter(|t| !t.is_empty())
        .unwrap_or(base_sha)
        .to_string()
}
