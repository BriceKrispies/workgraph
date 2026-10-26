//! wg-pynum: CPython's numeric and ordering semantics, shared by the Core crates.
//!
//! Python's `sum()` over floats is compensated summation (Neumaier), and `statistics.median` and
//! `bisect` have exact definitions that Rust's standard library does not share. The Core crates
//! (`wg-analytics`, `wg-experiments`, ...) call these instead of copying them. This crate is `Pure`:
//! it may use local mutation for sorting, but it does no I/O and reads no clock.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// CPython 3.12+ `sum()` over floats: Neumaier compensated summation starting from `0`.
///
/// Mirrors `builtin_sum_impl`: each term is added with the compensation term
/// `(f - t) + x` or `(x - t) + f` chosen by magnitude, and the compensation is added once at
/// the end when it is finite and non-zero.
#[must_use]
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let (total, compensation) = values
        .into_iter()
        .fold((0.0_f64, 0.0_f64), |(total, c), x| {
            let t = total + x;
            let delta = if total.abs() >= x.abs() {
                (total - t) + x
            } else {
                (x - t) + total
            };
            (t, c + delta)
        });
    if compensation != 0.0 && compensation.is_finite() {
        total + compensation
    } else {
        total
    }
}

/// `statistics.median` over floats: the middle value of the sorted data, or the mean of the two
/// middle values when there is an even count. `None` over no values (Python raises there; the
/// callers guard it).
#[must_use]
pub fn median(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = values.into_iter().collect();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n == 0 {
        None
    } else if n % 2 == 1 {
        sorted.get(n / 2).copied()
    } else {
        let high = n / 2;
        match (sorted.get(high - 1), sorted.get(high)) {
            (Some(low), Some(high)) => Some((low + high) / 2.0),
            _ => None,
        }
    }
}

/// A stable sort by `cmp`: equal elements keep their input order, as Python's `list.sort` does.
/// For `reverse=True` in Python, pass the reversed comparison.
#[must_use]
pub fn stable_sort_by<T>(
    items: impl IntoIterator<Item = T>,
    cmp: impl FnMut(&T, &T) -> Ordering,
) -> Vec<T> {
    let mut sorted: Vec<T> = items.into_iter().collect();
    sorted.sort_by(cmp);
    sorted
}

/// Groups `(key, value)` pairs by key, in ascending key order; each group keeps its values in
/// input order. This is the iteration order of a `GROUP BY` with an `ORDER BY` on the key.
#[must_use]
pub fn group_sorted<K: Ord, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Vec<(K, Vec<V>)> {
    let mut groups: BTreeMap<K, Vec<V>> = BTreeMap::new();
    for (key, value) in pairs {
        groups.entry(key).or_default().push(value);
    }
    groups.into_iter().collect()
}

/// The distinct values in ascending order (`sorted(set(items))`).
#[must_use]
pub fn sorted_distinct<T: Ord>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    items
        .into_iter()
        .collect::<BTreeSet<T>>()
        .into_iter()
        .collect()
}

/// `bisect.bisect_left` on a sorted slice: the first index whose value is not less than `x`.
#[must_use]
pub fn bisect_left(sorted: &[f64], x: f64) -> usize {
    sorted.partition_point(|value| *value < x)
}

/// `bisect.bisect_right` on a sorted slice: the first index whose value is greater than `x`.
#[must_use]
pub fn bisect_right(sorted: &[f64], x: f64) -> usize {
    sorted.partition_point(|value| *value <= x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_sum_compensates_like_cpython() {
        // Naive left-to-right summation of these loses the 1.0; the compensated sum keeps it.
        assert_eq!(py_sum([1e16, 1.0, -1e16]), 1.0);
        assert_eq!(py_sum(std::iter::empty()), 0.0);
    }

    #[test]
    fn median_odd_and_even() {
        assert_eq!(median([3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median([4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(std::iter::empty()), None);
    }

    #[test]
    fn stable_sort_keeps_ties_in_order() {
        let sorted = stable_sort_by([(1, 'a'), (0, 'b'), (1, 'c')], |x, y| y.0.cmp(&x.0));
        assert_eq!(sorted, vec![(1, 'a'), (1, 'c'), (0, 'b')]);
    }

    #[test]
    fn bisect_matches_python() {
        let values = [1.0, 2.0, 2.0, 3.0];
        assert_eq!(bisect_left(&values, 2.0), 1);
        assert_eq!(bisect_right(&values, 2.0), 3);
        assert_eq!(bisect_right(&values, 0.5), 0);
    }

    #[test]
    fn groups_keep_input_order() {
        let groups = group_sorted([("b", 1), ("a", 2), ("b", 3)]);
        assert_eq!(groups, vec![("a", vec![2]), ("b", vec![1, 3])]);
    }
}
