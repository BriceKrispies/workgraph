//! Classifier evaluation against a gold set (`workgraph.classification.service.compute_metrics`):
//! accuracy, per-class precision, recall and F1, macro F1 and the confusion matrix.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::order::{py_sum, round_to};

/// The confusion column for a classification that failed (no predicted category).
const FAILED: &str = "(failed)";

/// Metrics over `(expected, predicted)` pairs; `predicted` is `None` when the classification
/// failed. Ratios are rounded to four places; undefined ratios are `null`.
#[must_use]
pub fn compute_metrics(pairs: &[(String, Option<String>)]) -> Value {
    let labels: Vec<String> = pairs
        .iter()
        .map(|(expected, _)| expected.clone())
        .chain(
            pairs
                .iter()
                .filter_map(|(_, predicted)| predicted.clone())
                .filter(|predicted| !predicted.is_empty()),
        )
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let columns: Vec<String> = labels
        .iter()
        .cloned()
        .chain(std::iter::once(FAILED.to_string()))
        .collect();
    let column_of = |predicted: &Option<String>| -> String {
        predicted
            .as_deref()
            .filter(|text| !text.is_empty())
            .unwrap_or(FAILED)
            .to_string()
    };
    let count = |expected: &str, column: &str| {
        pairs
            .iter()
            .filter(|(actual, predicted)| actual == expected && column_of(predicted) == column)
            .count()
    };

    let per_class: Vec<(String, Value, Option<f64>)> = labels
        .iter()
        .map(|label| {
            let true_positive = count(label, label);
            let false_positive: usize = labels
                .iter()
                .filter(|other| other != &label)
                .map(|other| count(other, label))
                .sum();
            let false_negative: usize = columns
                .iter()
                .filter(|column| column != &label)
                .map(|column| count(label, column))
                .sum();
            let precision = ratio(true_positive, true_positive + false_positive);
            let recall = ratio(true_positive, true_positive + false_negative);
            let f1 = if precision + recall != 0.0 {
                2.0 * precision * recall / (precision + recall)
            } else {
                0.0
            };
            let support = true_positive + false_negative;
            let entry = json!({
                "precision": round_to(precision, 4),
                "recall": round_to(recall, 4),
                "f1": round_to(f1, 4),
                "support": support,
            });
            (label.clone(), entry, (support > 0).then_some(f1))
        })
        .collect();

    let correct = pairs
        .iter()
        .filter(|(expected, predicted)| predicted.as_deref() == Some(expected.as_str()))
        .count();
    let accuracy = if pairs.is_empty() {
        Value::Null
    } else {
        json!(round_to(correct as f64 / pairs.len() as f64, 4))
    };
    let f1s: Vec<f64> = per_class.iter().filter_map(|(_, _, f1)| *f1).collect();
    let macro_f1 = if f1s.is_empty() {
        Value::Null
    } else {
        let mean = py_sum(f1s.iter().copied()) / f1s.len() as f64;
        json!(round_to(mean, 4))
    };

    let per_class_map: Map<String, Value> = per_class
        .into_iter()
        .map(|(label, entry, _)| (label, entry))
        .collect();
    let confusion: Map<String, Value> = labels
        .iter()
        .map(|expected| {
            let row: Map<String, Value> = columns
                .iter()
                .map(|column| (column.clone(), json!(count(expected, column))))
                .collect();
            (expected.clone(), Value::Object(row))
        })
        .collect();
    json!({
        "accuracy": accuracy,
        "macro_f1": macro_f1,
        "per_class": per_class_map,
        "confusion": confusion,
    })
}

/// `numerator / denominator`, or 0 when the denominator is 0.
fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_of_a_small_gold_set() {
        // Hand-checked against the Python formulas: bug_fix has precision and recall 0.5, feature
        // has 0, so macro F1 is 0.25; the failed classification counts as a miss.
        let pairs = vec![
            ("bug_fix".to_string(), Some("bug_fix".to_string())),
            ("bug_fix".to_string(), Some("feature".to_string())),
            ("feature".to_string(), Some("bug_fix".to_string())),
            ("feature".to_string(), None),
        ];
        let metrics = compute_metrics(&pairs);
        assert_eq!(metrics["accuracy"], json!(0.25));
        assert_eq!(metrics["macro_f1"], json!(0.25));
        assert_eq!(metrics["per_class"]["bug_fix"]["support"], json!(2));
        assert_eq!(metrics["confusion"]["feature"]["(failed)"], json!(1));
        assert_eq!(metrics["per_class"]["bug_fix"]["precision"], json!(0.5));
    }

    #[test]
    fn empty_pairs_have_no_ratios() {
        let metrics = compute_metrics(&[]);
        assert_eq!(metrics["accuracy"], Value::Null);
        assert_eq!(metrics["macro_f1"], Value::Null);
    }
}
