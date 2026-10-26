//! Bisection of a failing integration tip: the merge sequence as entries, the commit of each
//! repository after a prefix of it, the search over prefixes, and the decision taken before the
//! search starts. Port of `_Entry`, `_sequence`, `_prefix_shas` and the decision steps of
//! `StackVerifier._bisect` in `integrations/git/stack_verification.py`.
//!
//! The shell checks out each probed prefix and runs the failing checks there; these functions say
//! which prefix to probe next and what the probes prove.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use wg_types::{ErrorCode, WgError};
use wg_verify::Baseline;

use crate::item::is_on_branch;
use crate::ordered::{sorted, sorted_by};
use crate::queue::{ItemDetail, QueueItem};
use crate::regression::{
    BaseOutcomes, Regressions, RunCheck, errored_checks, failed_checks, is_absolute, regressions,
    sorted_names,
};

/// One stack item with the ids and title the verifier needs.
#[derive(Debug, Clone, PartialEq)]
pub struct StackRow {
    pub item_id: String,
    pub task_id: String,
    pub title: String,
    pub item: QueueItem,
}

/// One item on the integration branch, in merge order (`_Entry`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub item_id: String,
    pub task_id: String,
    pub task_key: String,
    pub title: String,
    /// Repository name to `{head, merge_commit}`, in the order the detail stores them.
    pub merges: Map<String, Value>,
}

impl Entry {
    /// The merge commit recorded for `repository`.
    pub fn merge_commit(&self, repository: &str) -> Result<&str, WgError> {
        self.merge_field(repository, "merge_commit")
    }

    /// The head that was merged into `repository`.
    pub fn head(&self, repository: &str) -> Result<&str, WgError> {
        self.merge_field(repository, "head")
    }

    fn merge_field(&self, repository: &str, field: &str) -> Result<&str, WgError> {
        self.merges
            .get(repository)
            .and_then(|merge| merge.get(field))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WgError::new(
                    ErrorCode::WorkgraphError,
                    format!(
                        "{}'s merge record for {repository} has no {field}",
                        self.task_key
                    ),
                )
            })
    }
}

/// `_sequence`: the items whose merges are on the integration branch, in merge order.
pub fn sequence(rows: &[StackRow]) -> Vec<Entry> {
    ordered_rows(rows)
        .into_iter()
        .filter(|row| is_on_branch(row.item.state))
        .filter_map(|row| {
            let merges = ItemDetail::load(row.item.detail.as_deref()).merges;
            (!merges.is_empty()).then(|| Entry {
                item_id: row.item_id.clone(),
                task_id: row.task_id.clone(),
                task_key: row.item.key.clone(),
                title: row.title.clone(),
                merges,
            })
        })
        .collect()
}

/// The rows in `item_rows` order, the order [`crate::queue::ordered_items`] gives their items.
pub fn ordered_rows(rows: &[StackRow]) -> Vec<&StackRow> {
    sorted_by(rows.iter(), |row| {
        (
            row.item.merge_order.is_none(),
            row.item.merge_order.unwrap_or(0),
            row.item.number,
        )
    })
}

/// `_prefix_shas`: the commit each repository holds after the first `position` items of the
/// merge sequence. `bases` maps repository names to the stack's base commits.
pub fn prefix_shas(
    bases: &BTreeMap<String, String>,
    sequence: &[Entry],
    position: usize,
) -> Result<BTreeMap<String, String>, WgError> {
    let prefix = &sequence[..position.min(sequence.len())];
    // Every merge of the prefix must name its commit, as the Python loop reads each one.
    prefix.iter().try_for_each(|entry| {
        entry
            .merges
            .keys()
            .try_for_each(|name| entry.merge_commit(name).map(|_| ()))
    })?;
    let names: BTreeSet<&String> = bases
        .keys()
        .chain(prefix.iter().flat_map(|entry| entry.merges.keys()))
        .collect();
    names
        .into_iter()
        .map(|name| {
            let last = prefix
                .iter()
                .rev()
                .find(|entry| entry.merges.contains_key(name));
            let sha = match last {
                Some(entry) => entry.merge_commit(name)?.to_string(),
                None => bases.get(name).cloned().unwrap_or_default(),
            };
            Ok((name.clone(), sha))
        })
        .collect()
}

/// The decision taken when the tip has regressions, before any prefix is probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BisectEntry {
    /// Nothing is ejected: there is no merged change, or an absolute check fails on the base
    /// itself. The stack returns to `integrating`.
    FailedOnBase {
        /// `no merged change to eject` or `the base fails`.
        reason: &'static str,
        /// The absolute checks failing on the base, else every failing check; sorted.
        failed_checks: Vec<String>,
        /// The tip run when there is no sequence, else the base run of the first base failure.
        run_id: Option<String>,
    },
    /// Bisect the merge sequence.
    Search,
}

/// The start of `_bisect`: whether the failure belongs to the base rather than to any child.
///
/// `tip_run_id` and `tip_checks` are the failing run on the tip; `base` holds the outcomes of the
/// failed checks on the base.
pub fn bisect_entry(
    sequence_len: usize,
    tip_run_id: &str,
    tip_checks: &[RunCheck],
    base: &BaseOutcomes,
    baselines: &BTreeMap<String, Baseline>,
) -> BisectEntry {
    let on_base = base_failures(tip_checks, base, baselines);
    if sequence_len > 0 && on_base.is_empty() {
        return BisectEntry::Search;
    }
    let (reason, run_id) = if sequence_len == 0 {
        ("no merged change to eject", Some(tip_run_id.to_string()))
    } else {
        let first = on_base.first().and_then(|name| base.get(name));
        ("the base fails", first.and_then(|o| o.run_id.clone()))
    };
    let failed_checks = if on_base.is_empty() {
        sorted_names(&regressions(tip_checks, Some(base), baselines))
    } else {
        on_base
    };
    BisectEntry::FailedOnBase {
        reason,
        failed_checks,
        run_id,
    }
}

/// Absolute checks that failed on the tip and did not pass on the base (a failure of the base
/// itself), sorted.
pub fn base_failures(
    tip_checks: &[RunCheck],
    base: &BaseOutcomes,
    baselines: &BTreeMap<String, Baseline>,
) -> Vec<String> {
    sorted(
        failed_checks(tip_checks)
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| {
                is_absolute(baselines, name)
                    && base
                        .get(name)
                        .is_none_or(|o| o.status != wg_types::VerificationStatus::Passed)
            }),
    )
}

/// The search over prefixes. Invariant: the prefix `low` passes and the prefix `high` fails; the
/// base (prefix 0) passes by construction and the whole sequence (the tip) fails. `R` is the run
/// that proved `high` fails, which the ejection reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bisection<R> {
    low: usize,
    high: usize,
    high_run: R,
}

impl<R> Bisection<R> {
    /// A search over a sequence of `len` items whose tip failed in `tip_run`.
    pub fn new(len: usize, tip_run: R) -> Self {
        Self {
            low: 0,
            high: len,
            high_run: tip_run,
        }
    }

    /// The prefix to probe next, or `None` once the failing item is found.
    pub fn next_probe(&self) -> Option<usize> {
        (self.high - self.low > 1).then_some((self.low + self.high) / 2)
    }

    /// The search after the probe of prefix `position` passed or failed in `run`.
    pub fn record(self, position: usize, passed: bool, run: R) -> Self {
        if passed {
            Self {
                low: position,
                ..self
            }
        } else {
            Self {
                high: position,
                high_run: run,
                ..self
            }
        }
    }

    /// The lowest failing prefix: the culprit is item `position - 1` of the sequence.
    pub fn position(&self) -> usize {
        self.high
    }

    /// The run that proved [`Bisection::position`] fails.
    pub fn high_run(&self) -> &R {
        &self.high_run
    }

    /// The found position and its failing run.
    pub fn finish(self) -> (usize, R) {
        (self.high, self.high_run)
    }
}

/// What one probe proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// A required check could not judge the prefix: the verification stops with stage `bisect`.
    Errored(Vec<String>),
    /// No failure counts against the prefix.
    Passed,
    /// The prefix has regressions.
    Failed(Regressions),
}

/// Judges one probe run against the base.
pub fn probe_verdict(
    checks: &[RunCheck],
    base: &BaseOutcomes,
    baselines: &BTreeMap<String, Baseline>,
) -> ProbeVerdict {
    let errored = errored_checks(checks);
    if !errored.is_empty() {
        return ProbeVerdict::Errored(sorted(errored.iter().map(|c| c.name.clone())));
    }
    let found = regressions(checks, Some(base), baselines);
    if found.is_empty() {
        ProbeVerdict::Passed
    } else {
        ProbeVerdict::Failed(found)
    }
}

/// The required checks of a probe that did not pass, sorted (the `stack.bisect_probe` event).
pub fn probe_failed_checks(checks: &[RunCheck]) -> Vec<String> {
    sorted(
        checks
            .iter()
            .filter(|c| c.required && c.status != wg_types::VerificationStatus::Passed)
            .map(|c| c.name.clone()),
    )
}
