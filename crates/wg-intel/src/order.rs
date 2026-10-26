//! Stable ordering without mutation, for the Core tier (no `mut` bindings).

/// Sorts `items` so that `before` holds between each element and the ones after it, keeping the
/// input order of elements that are not strictly ordered (the stability of Python's `sorted`).
pub(crate) fn sorted_by<T: Clone>(
    items: impl IntoIterator<Item = T>,
    before: impl Fn(&T, &T) -> bool,
) -> Vec<T> {
    items.into_iter().fold(Vec::new(), |sorted, item| {
        let at = sorted
            .iter()
            .position(|existing| before(&item, existing))
            .unwrap_or(sorted.len());
        sorted[..at]
            .iter()
            .cloned()
            .chain(std::iter::once(item))
            .chain(sorted[at..].iter().cloned())
            .collect()
    })
}

/// Python 3.12's `sum()` over floats: Neumaier compensated summation, then the compensation is
/// added back. A non-finite total is returned as it is.
pub(crate) fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let (total, compensation) = values
        .into_iter()
        .fold((0.0_f64, 0.0_f64), |(total, c), x| {
            let sum = total + x;
            let c = if total.abs() >= x.abs() {
                c + ((total - sum) + x)
            } else {
                c + ((x - sum) + total)
            };
            (sum, c)
        });
    if total.is_finite() {
        total + compensation
    } else {
        total
    }
}

/// Round half to even on the exact binary value, as Python's `round(x, digits)` does.
pub(crate) fn round_to(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// The first occurrence of each item, in order (`dict.fromkeys(items)`).
pub(crate) fn unique<T: Clone + PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    items.into_iter().fold(Vec::new(), |seen, item| {
        if seen.contains(&item) {
            seen
        } else {
            seen.into_iter().chain(std::iter::once(item)).collect()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_by_is_stable_for_equal_keys() {
        let items = vec![("a", 2), ("b", 1), ("c", 2), ("d", 3)];
        let sorted = sorted_by(items, |x, y| x.1 > y.1);
        assert_eq!(sorted, vec![("d", 3), ("a", 2), ("c", 2), ("b", 1)]);
    }

    #[test]
    fn round_to_rounds_half_to_even_on_the_binary_value() {
        // Python: round(0.125, 2) == 0.12 and round(0.375, 2) == 0.38
        assert_eq!(round_to(0.125, 2), 0.12);
        assert_eq!(round_to(0.375, 2), 0.38);
    }

    #[test]
    fn unique_keeps_first_occurrences() {
        assert_eq!(unique(["b", "a", "b", "c", "a"]), vec!["b", "a", "c"]);
    }
}
