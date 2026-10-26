//! Similar-task candidates (`SearchService.similar`): which tasks are compared with a target, and
//! with what similarity. Semantic when the target has a stored vector under the selected model,
//! lexical otherwise.

use wg_types::Id;

use crate::search::cosine;

/// The method of a similarity lookup and its candidates, in the order Python inserts them.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidates {
    /// `semantic` when the target has a vector, else `lexical`.
    pub method: &'static str,
    /// The other tasks and their similarity to the target.
    pub similar: Vec<(Id, f64)>,
}

/// The candidates of `target`. `mine` is the target's vector when it has one; `vectors` are every
/// stored vector of the workspace under the model; `lexical` are the lexical hits in rank order.
/// A lexical similarity is `round(1 / rank, 4)`, counting the target's own rank.
#[must_use]
pub fn candidates(
    target: Id,
    mine: Option<&[f64]>,
    vectors: &[(Id, Vec<f64>)],
    lexical: &[Id],
) -> Candidates {
    match mine {
        Some(mine) => Candidates {
            method: "semantic",
            similar: vectors
                .iter()
                .filter(|(id, _)| *id != target)
                .map(|(id, vector)| (*id, cosine(mine, vector)))
                .collect(),
        },
        None => Candidates {
            method: "lexical",
            similar: lexical
                .iter()
                .enumerate()
                .filter(|(_, id)| **id != target)
                .map(|(at, id)| (*id, round4(1.0 / (at + 1) as f64)))
                .collect(),
        },
    }
}

/// `round(x, 4)` as Python rounds it.
#[must_use]
pub fn round4(value: f64) -> f64 {
    format!("{value:.4}").parse().unwrap_or(value)
}
