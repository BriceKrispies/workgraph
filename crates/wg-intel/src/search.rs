//! Cosine similarity and reciprocal rank fusion, as `SearchService` computes them.

use rpds::RedBlackTreeMap;

use crate::order::{py_sum, round_to, sorted_by, unique};

/// The damping constant of reciprocal rank fusion: `score = sum(1 / (RRF_K + rank))`.
pub const RRF_K: u32 = 60;

/// One task in a fused ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    pub task_id: String,
    pub score: f64,
    /// 1-based rank in the lexical list (the last occurrence), if the task was matched lexically.
    pub lexical_rank: Option<usize>,
    /// 1-based rank in the semantic list (the last occurrence), if the task was matched by meaning.
    pub semantic_rank: Option<usize>,
}

/// Cosine similarity; 0 for vectors of different lengths or with a zero norm.
#[must_use]
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot = py_sum(a.iter().zip(b).map(|(x, y)| x * y));
    let norm_a = py_sum(a.iter().map(|x| x * x)).sqrt();
    let norm_b = py_sum(b.iter().map(|y| y * y)).sqrt();
    if norm_a != 0.0 && norm_b != 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

/// Fuses lexical hits `(task id, bm25)` and semantic hits `(task id, similarity)`, each best
/// first. Without semantic hits the lexical list is returned as it is, scored by its negated bm25.
/// Otherwise the tasks are ordered by summed reciprocal ranks, ties keeping first-seen order.
#[must_use]
pub fn fuse(lexical: &[(String, f64)], semantic: &[(String, f64)], limit: usize) -> Vec<FusedHit> {
    if semantic.is_empty() {
        return lexical
            .iter()
            .take(limit)
            .enumerate()
            .map(|(index, (task_id, bm25))| FusedHit {
                task_id: task_id.clone(),
                score: round_to(-bm25, 4),
                lexical_rank: Some(index + 1),
                semantic_rank: None,
            })
            .collect();
    }
    let contributions = ranked_ids(lexical).chain(ranked_ids(semantic));
    let scores = contributions.fold(
        RedBlackTreeMap::<String, f64>::new(),
        |scores, (task_id, rank)| {
            let total = scores.get(task_id).copied().unwrap_or(0.0)
                + 1.0 / (f64::from(RRF_K) + rank as f64);
            scores.insert(task_id.to_string(), total)
        },
    );
    let order = unique(
        lexical
            .iter()
            .chain(semantic)
            .map(|(task_id, _)| task_id.as_str()),
    );
    let hits: Vec<FusedHit> = order
        .into_iter()
        .map(|task_id| FusedHit {
            task_id: task_id.to_string(),
            score: scores.get(task_id).copied().unwrap_or(0.0),
            lexical_rank: last_rank(lexical, task_id),
            semantic_rank: last_rank(semantic, task_id),
        })
        .collect();
    sorted_by(hits, |a, b| a.score > b.score)
        .into_iter()
        .take(limit)
        .map(|hit| FusedHit {
            score: round_to(hit.score, 6),
            ..hit
        })
        .collect()
}

/// `(task id, 1-based rank)` for each entry of a ranked list.
fn ranked_ids(hits: &[(String, f64)]) -> impl Iterator<Item = (&str, usize)> {
    hits.iter()
        .enumerate()
        .map(|(index, (task_id, _))| (task_id.as_str(), index + 1))
}

/// The 1-based rank of the last entry for `task_id`, if it is listed.
fn last_rank(hits: &[(String, f64)], task_id: &str) -> Option<usize> {
    hits.iter()
        .enumerate()
        .rev()
        .find(|(_, (listed, _))| listed.as_str() == task_id)
        .map(|(index, _)| index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, f64)]) -> Vec<(String, f64)> {
        items
            .iter()
            .map(|(id, value)| (id.to_string(), *value))
            .collect()
    }

    #[test]
    fn a_task_in_both_lists_outranks_single_matches() {
        let lexical = pairs(&[("a", -3.0), ("b", -2.0)]);
        let semantic = pairs(&[("c", 0.9), ("b", 0.8)]);
        let fused = fuse(&lexical, &semantic, 10);
        assert_eq!(fused[0].task_id, "b");
        assert_eq!(fused[0].lexical_rank, Some(2));
        assert_eq!(fused[0].semantic_rank, Some(2));
        assert_eq!(fused[1].task_id, "a");
        assert_eq!(fused[1].semantic_rank, None);
    }

    #[test]
    fn cosine_is_zero_for_mismatched_or_zero_vectors() {
        assert_eq!(cosine(&[1.0, 2.0], &[1.0, 2.0, 3.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
