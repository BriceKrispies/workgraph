//! Regression judgement: which failures of a verification run count against a stack, given how the
//! same checks finished on the stack's base. Port of `regressions` and `_leaf` in
//! `integrations/git/regression.py`, and of `_failed_checks`, `_errored_checks` and `_is_absolute`
//! in `integrations/git/stack_verification.py`.
//!
//! A check with `baseline="absolute"` fails whenever it does not pass. A check with
//! `baseline="regression"` fails only on signatures the base does not already report; a timeout or
//! a failure without any parsed signature cannot be compared and fails. A check with status
//! `error` judged nothing: it fails in either mode and is never compared with the base. A test
//! failure is identified by its test function (`test:a::b::name` matches a base `test:name`), so a
//! test that moved to another binary keeps its known failure.
//!
//! Python dictionaries keep insertion order and a repeated key keeps its first position; the
//! ordered pair lists here ([`Regressions`], [`BaseOutcomes`]) do the same.

use std::collections::{BTreeMap, BTreeSet};

use wg_types::VerificationStatus;
use wg_verify::Baseline;

use crate::ordered::{dict_from, dict_with, sorted};

/// One check result of a verification run, as the stack judgement reads it (`CheckResultOut`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCheck {
    pub name: String,
    pub required: bool,
    pub status: VerificationStatus,
    /// The repository the result was recorded against; `None` for integration checks.
    pub repository_id: Option<String>,
    pub failure_signatures: Vec<String>,
    /// The tail of the check's output, kept for errored-check reports.
    pub output_excerpt: String,
}

/// A finished verification run: its id, whether every required check passed, and its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub id: String,
    pub required_passed: bool,
    pub checks: Vec<RunCheck>,
}

/// How one check finished on a revision (`CheckOutcome`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub status: VerificationStatus,
    pub signatures: Vec<String>,
    /// The run the outcome was taken from; it never affects the judgement.
    pub run_id: Option<String>,
    pub excerpt: String,
}

/// Failing checks with the signatures that count against them, in the order of the run's checks.
pub type Regressions = Vec<(String, Vec<String>)>;

/// The base outcome of each check name, in the order they were found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BaseOutcomes {
    entries: Vec<(String, CheckOutcome)>,
}

impl BaseOutcomes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the map from pairs; a repeated name replaces the earlier outcome in place.
    pub fn from_pairs(pairs: Vec<(String, CheckOutcome)>) -> Self {
        pairs
            .into_iter()
            .fold(Self::new(), |map, (name, outcome)| map.with(name, outcome))
    }

    /// The map with `name` set to `outcome`, keeping the name's first position.
    pub fn with(self, name: String, outcome: CheckOutcome) -> Self {
        Self {
            entries: dict_with(self.entries, &name, outcome),
        }
    }

    pub fn get(&self, name: &str) -> Option<&CheckOutcome> {
        self.entries
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, outcome)| outcome)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The outcomes in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CheckOutcome)> {
        self.entries
            .iter()
            .map(|(name, outcome)| (name.as_str(), outcome))
    }
}

/// `regressions`: the failures of `checks` that count against the run, keyed by check name.
///
/// Only required checks can fail a run. A check missing from `baselines` (the check specs, by
/// name) is judged absolutely. `base` is `None` when no base outcome was gathered.
pub fn regressions(
    checks: &[RunCheck],
    base: Option<&BaseOutcomes>,
    baselines: &BTreeMap<String, Baseline>,
) -> Regressions {
    checks
        .iter()
        .filter(|check| check.required && check.status != VerificationStatus::Passed)
        .fold(Vec::new(), |found, check| {
            match counted(check, base, baselines) {
                Some(signatures) => dict_with(found, &check.name, signatures),
                None => found,
            }
        })
}

/// The signatures of a required, not passing check that count against the run, or `None` when
/// the base already reports all of them.
fn counted(
    check: &RunCheck,
    base: Option<&BaseOutcomes>,
    baselines: &BTreeMap<String, Baseline>,
) -> Option<Vec<String>> {
    let signatures = &check.failure_signatures;
    let compared = check.status != VerificationStatus::Error
        && baselines.get(&check.name) == Some(&Baseline::Regression)
        && !signatures.is_empty()
        && !signatures.iter().any(|s| s == "timeout");
    if !compared {
        return Some(signatures.clone());
    }
    let outcome = base
        .and_then(|b| b.get(&check.name))
        .filter(|o| o.status != VerificationStatus::Passed);
    let Some(outcome) = outcome else {
        return Some(signatures.clone());
    };
    let known: BTreeSet<String> = outcome
        .signatures
        .iter()
        .cloned()
        .chain(outcome.signatures.iter().map(|s| leaf(s)))
        .collect();
    let new: Vec<String> = signatures
        .iter()
        .filter(|s| !known.contains(*s) && !known.contains(&leaf(s)))
        .cloned()
        .collect();
    (!new.is_empty()).then_some(new)
}

/// `_leaf`: `test:a::b::name` becomes `test:name`; other signatures are returned unchanged.
pub fn leaf(signature: &str) -> String {
    match signature.strip_prefix("test:") {
        Some(rest) => format!("test:{}", rest.rsplit("::").next().unwrap_or(rest)),
        None => signature.to_string(),
    }
}

/// `_failed_checks`: required checks that did not pass, each with the repository its result was
/// recorded against.
pub fn failed_checks(checks: &[RunCheck]) -> Vec<(String, Option<String>)> {
    dict_from(
        checks
            .iter()
            .filter(|check| check.required && check.status != VerificationStatus::Passed)
            .map(|check| (check.name.clone(), check.repository_id.clone())),
    )
}

/// `_errored_checks`: required checks that could not judge anything. A run with one proves
/// nothing about its revision.
pub fn errored_checks(checks: &[RunCheck]) -> Vec<&RunCheck> {
    checks
        .iter()
        .filter(|c| c.required && c.status == VerificationStatus::Error)
        .collect()
}

/// `_is_absolute`: a check without a spec is judged absolutely.
pub fn is_absolute(baselines: &BTreeMap<String, Baseline>, name: &str) -> bool {
    baselines
        .get(name)
        .is_none_or(|baseline| *baseline == Baseline::Absolute)
}

/// The names of `regressions`, sorted (`sorted(failing)`).
pub fn sorted_names(found: &[(String, Vec<String>)]) -> Vec<String> {
    sorted(found.iter().map(|(name, _)| name.clone()))
}
