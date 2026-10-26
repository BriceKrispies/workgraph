//! Order statistics and sums with the semantics of the DuckDB aggregates the metrics use:
//! `quantile_cont` (linear interpolation between the two nearest ranks), `AVG`, `SUM`, `MIN` and
//! `MAX`. Each is NULL over no values, as in SQL, so the functions return `Option`.

use std::cmp::Ordering;

use rpds::{RedBlackTreeMapSync, VectorSync};

/// A float ordered by `total_cmp`, so that it can key a persistent map.
#[derive(Debug, Clone, Copy)]
pub struct OrdF64(pub f64);

impl PartialEq for OrdF64 {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// The values in ascending order, duplicates kept. The values are counted in a persistent map
/// first, so the sort needs no mutable buffer.
pub fn sorted(values: &VectorSync<f64>) -> Vec<f64> {
    values
        .iter()
        .fold(
            RedBlackTreeMapSync::<OrdF64, usize>::new_sync(),
            |counts, value| {
                let key = OrdF64(*value);
                let count = counts.get(&key).copied().unwrap_or(0);
                counts.insert(key, count + 1)
            },
        )
        .iter()
        .flat_map(|(key, count)| std::iter::repeat_n(key.0, *count))
        .collect()
}

/// `quantile_cont(values, q)` over ascending `sorted` values: the value at rank `q * (n - 1)`,
/// interpolated linearly between the two neighbouring ranks. NULL over no values.
pub fn quantile_cont(sorted: &[f64], q: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let position = q * last as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    let (below, above) = (*sorted.get(low)?, *sorted.get(high)?);
    let fraction = position - low as f64;
    Some(below * (1.0 - fraction) + above * fraction)
}

/// `AVG`: the sum divided by the count, NULL over no values.
pub fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// `SUM`: the sum of the values, NULL over no values.
pub fn sum(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum())
}

/// `SUM` of two nullable partial sums: NULLs are skipped, and the result is NULL only when both
/// sides are.
pub fn add_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Rounds to six decimal places, as the service rounds every float it returns (`_jsonable`).
pub fn round6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}
