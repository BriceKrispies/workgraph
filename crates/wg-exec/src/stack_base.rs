//! Stack base selection (CORE-13): where a stack child's attempt starts in one repository.
//!
//! Mirrors `stack_base` and `builds_on_others` in `core/services/stacks.py`. The shell reads the
//! stack item state, the sibling heads recorded at submit and the commit graph; this module
//! decides which commit the attempt starts from. Commit ancestry is passed as the parent map of
//! the commit graph, so every decision is a pure function of its inputs.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use wg_types::StackItemState;

/// A stack sibling of the child, with the head it left in the repository (if it touched it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub key: String,
    pub number: i64,
    pub head: Option<String>,
}

/// A sibling head that the chosen sibling supersedes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superseded {
    pub task: String,
    pub head: String,
}

/// The outcome of choosing among sibling heads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiblingChoice {
    /// No sibling touched the repository: the attempt starts from the stack base.
    StackBase,
    /// The latest sibling's head is the start; the others are superseded.
    SiblingHead {
        sibling: String,
        sibling_head: String,
        superseded: Vec<Superseded>,
    },
    /// Two sibling heads are not one line of history: the child cannot choose between them.
    /// Names the first pair (in sibling order) that diverges.
    Diverged { first: String, second: String },
}

/// Where a stack child's attempt starts in one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseChoice {
    /// The child was sent back for a conflict or ejected: it starts from the integration tip.
    IntegrationTip,
    /// The child starts from the stack base or a sibling head, as decided by `SiblingChoice`.
    Siblings(SiblingChoice),
}

/// Whether a child in `state` starts from the integration tip (`stack_base`, first branch).
pub fn starts_from_integration_tip(state: StackItemState) -> bool {
    matches!(state, StackItemState::Conflict | StackItemState::Ejected)
}

/// Chooses the base for a child in `state`, given its siblings and the commit graph.
pub fn choose_base(
    state: StackItemState,
    siblings: &[Sibling],
    parents: &BTreeMap<String, Vec<String>>,
) -> BaseChoice {
    if starts_from_integration_tip(state) {
        BaseChoice::IntegrationTip
    } else {
        BaseChoice::Siblings(choose_sibling(siblings, parents))
    }
}

/// Whether `ancestor` is reachable from `descendant`, counting a commit as its own ancestor
/// (`git merge-base --is-ancestor`).
pub fn is_ancestor(
    parents: &BTreeMap<String, Vec<String>>,
    ancestor: &str,
    descendant: &str,
) -> bool {
    // Breadth-first layers: each layer holds the commits reached for the first time at that depth.
    std::iter::successors(
        Some((vec![descendant], BTreeSet::<&str>::new())),
        |(frontier, seen)| {
            let fresh: BTreeSet<&str> = frontier
                .iter()
                .copied()
                .filter(|commit| !seen.contains(commit))
                .collect();
            (!fresh.is_empty()).then(|| {
                let next: Vec<&str> = fresh
                    .iter()
                    .filter_map(|commit| parents.get(*commit))
                    .flat_map(|ids| ids.iter().map(String::as_str))
                    .collect();
                (next, seen.union(&fresh).copied().collect())
            })
        },
    )
    .any(|(frontier, _)| frontier.contains(&ancestor))
}

/// Chooses the sibling whose head the child starts from (`builds_on_others` ordering).
///
/// Siblings without a head are ignored. Heads must form one line of history pairwise, checked in
/// sibling order. The head that builds on the most other heads wins; ties keep sibling order.
pub fn choose_sibling(
    siblings: &[Sibling],
    parents: &BTreeMap<String, Vec<String>>,
) -> SiblingChoice {
    let heads: Vec<(&str, &str)> = siblings
        .iter()
        .filter_map(|s| s.head.as_deref().map(|h| (s.key.as_str(), h)))
        .collect();
    if heads.is_empty() {
        return SiblingChoice::StackBase;
    }
    for (i, (first, first_head)) in heads.iter().enumerate() {
        for (second, second_head) in &heads[i + 1..] {
            if !(is_ancestor(parents, first_head, second_head)
                || is_ancestor(parents, second_head, first_head))
            {
                return SiblingChoice::Diverged {
                    first: (*first).to_owned(),
                    second: (*second).to_owned(),
                };
            }
        }
    }
    let builds_on_others = |&(key, head): &(&str, &str)| -> usize {
        heads
            .iter()
            .filter(|(other, other_head)| *other != key && is_ancestor(parents, other_head, head))
            .count()
    };
    // Descending by score; equal scores keep sibling order, as Python's sorted does.
    let scored: Vec<((&str, &str), usize)> = heads
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let score = builds_on_others(entry);
            ((Reverse(score), index), (*entry, score))
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let ((latest, head), _) = scored[0];
    let superseded = scored[1..]
        .iter()
        .map(|((task, head), _)| Superseded {
            task: (*task).to_owned(),
            head: (*head).to_owned(),
        })
        .collect();
    SiblingChoice::SiblingHead {
        sibling: latest.to_owned(),
        sibling_head: head.to_owned(),
        superseded,
    }
}
