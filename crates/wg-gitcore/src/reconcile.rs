//! Rewrite reconciliation and cherry-pick linking: which older commits of a repository carry the
//! same change as a newly ingested commit, and which task links a rewritten commit inherits.
//!
//! The shell loads the commit rows and writes what comes back; these functions decide. Ported
//! from `_detect_equivalences` and `_carry_links` in `integrations/git/service.py`.

use std::collections::BTreeSet;

use crate::cherry_picked_from;

/// Confidence of a link carried onto a rewritten commit (`AttributionSource.REWRITE_RECONCILIATION`).
pub const REWRITE_CONFIDENCE: f64 = 0.85;

/// The facts about one commit that the decisions read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFacts {
    pub id: String,
    pub sha: String,
    pub repository_id: String,
    pub patch_id: Option<String>,
    /// `None` when reachability is unknown; `Some(false)` once the commit left every ref.
    pub reachable: Option<bool>,
    pub author_email: String,
    /// Author date as Unix seconds.
    pub authored_at: Option<i64>,
    pub subject: String,
    pub message: String,
}

/// `rewrite` (the old commit was replaced) or `cherry_pick` (both still exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquivalenceKind {
    Rewrite,
    CherryPick,
}

impl EquivalenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewrite => "rewrite",
            Self::CherryPick => "cherry_pick",
        }
    }
}

/// How the pair was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EquivalenceMethod {
    PatchId { patch_id: String },
    AuthorDateSubject,
    CherryPickTrailer,
}

/// One equivalence to record between an older commit and a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Equivalence {
    pub original_id: String,
    pub equivalent_id: String,
    pub kind: EquivalenceKind,
    pub method: EquivalenceMethod,
    pub original_sha: String,
    pub new_sha: String,
}

/// Equivalences for each new commit, in the order of `new_ids`.
///
/// * same patch-id, older commit unreachable -> `rewrite` (rebase)
/// * same patch-id, older commit reachable -> `cherry_pick`
/// * otherwise, unreachable commit with the same author, author date and subject -> `rewrite` (amend)
/// * `(cherry picked from commit X)` in the message -> `cherry_pick`
///
/// `commits` is every commit of the repositories involved. `existing` holds the
/// `(original_id, equivalent_id)` pairs already recorded; those are not emitted again.
pub fn plan_equivalences(
    commits: &[CommitFacts],
    new_ids: &[String],
    existing: &[(String, String)],
) -> Vec<Equivalence> {
    let new_set: BTreeSet<&str> = new_ids.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for new_id in new_ids {
        let Some(commit) = commits.iter().find(|c| &c.id == new_id) else {
            continue;
        };
        let mut matches: Vec<(&CommitFacts, EquivalenceKind, EquivalenceMethod)> = Vec::new();

        if let Some(patch) = non_empty(&commit.patch_id) {
            for other in commits.iter().filter(|o| {
                o.repository_id == commit.repository_id
                    && o.patch_id.as_deref() == Some(patch)
                    && o.id != commit.id
            }) {
                // A twin that is itself a new commit counts only once it has left every ref.
                if new_set.contains(other.id.as_str()) && other.reachable != Some(false) {
                    continue;
                }
                let kind = if other.reachable == Some(true) {
                    EquivalenceKind::CherryPick
                } else {
                    EquivalenceKind::Rewrite
                };
                matches.push((
                    other,
                    kind,
                    EquivalenceMethod::PatchId {
                        patch_id: patch.to_owned(),
                    },
                ));
            }
        }

        if let Some(authored) = commit.authored_at
            && matches.is_empty()
        {
            for other in commits.iter().filter(|o| {
                o.repository_id == commit.repository_id
                    && o.id != commit.id
                    && o.reachable == Some(false)
                    && o.author_email == commit.author_email
                    && o.authored_at == Some(authored)
                    && o.subject == commit.subject
            }) {
                matches.push((
                    other,
                    EquivalenceKind::Rewrite,
                    EquivalenceMethod::AuthorDateSubject,
                ));
            }
        }

        // A trailer naming a commit already matched by patch id adds nothing.
        if let Some(prefix) = cherry_picked_from(&commit.message)
            && let Some(picked) = commits.iter().find(|o| {
                o.repository_id == commit.repository_id && o.sha.starts_with(prefix.as_str())
            })
            && !matches.iter().any(|(m, _, _)| m.id == picked.id)
        {
            matches.push((
                picked,
                EquivalenceKind::CherryPick,
                EquivalenceMethod::CherryPickTrailer,
            ));
        }

        for (other, kind, method) in matches {
            let known = existing
                .iter()
                .any(|(orig, equiv)| orig == &other.id && equiv == &commit.id);
            if known {
                continue;
            }
            out.push(Equivalence {
                original_id: other.id.clone(),
                equivalent_id: commit.id.clone(),
                kind,
                method,
                original_sha: other.sha.clone(),
                new_sha: commit.sha.clone(),
            });
        }
    }
    out
}

/// A task link on the original commit of a rewrite.
#[derive(Debug, Clone, PartialEq)]
pub struct OriginalLink {
    pub task_id: String,
    pub source: String,
    pub confidence: f64,
    pub attempt_id: Option<String>,
}

/// A link to write on the rewritten commit.
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedLink {
    pub task_id: String,
    pub confidence: f64,
    pub from_commit: String,
    pub original_source: String,
    pub attempt_id: Option<String>,
}

/// The `rewrite_reconciliation` links to write on the rewritten commit for each task link of the
/// original. `rewritten_tasks` lists the tasks already linked to the rewritten commit through
/// reconciliation; a task appears at most once.
pub fn carry_links(
    original_sha: &str,
    original_links: &[OriginalLink],
    rewritten_tasks: &[String],
) -> Vec<CarriedLink> {
    let mut seen: BTreeSet<&str> = rewritten_tasks.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for link in original_links {
        if !seen.insert(link.task_id.as_str()) {
            continue;
        }
        out.push(CarriedLink {
            task_id: link.task_id.clone(),
            confidence: link.confidence.min(REWRITE_CONFIDENCE),
            from_commit: original_sha.to_owned(),
            original_source: link.source.clone(),
            attempt_id: link.attempt_id.clone(),
        });
    }
    out
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}
