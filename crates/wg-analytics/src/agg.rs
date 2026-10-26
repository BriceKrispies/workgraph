//! Aggregation with SQL semantics, as the warehouse's `GROUP BY` and joins need it. `SUM`, `MIN`
//! and `MAX` skip NULLs and are NULL over no values; a group key that is NULL is a key like any
//! other in `GROUP BY`, while an equality join never matches it ([`lookup`]). Groups are
//! persistent maps, so they iterate in key order.

use rpds::RedBlackTreeMapSync;

/// Groups keyed in order.
pub(crate) type Groups<K, V> = RedBlackTreeMapSync<K, V>;

/// Groups `items` by `key`. The first row of a group starts its value with `of`, and each later
/// row is folded in with `merge`.
pub(crate) fn aggregate<T, K: Ord + Clone, V: Clone>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
    of: impl Fn(&T) -> V,
    merge: impl Fn(V, V) -> V,
) -> Groups<K, V> {
    items.into_iter().fold(Groups::new_sync(), |groups, item| {
        let k = key(&item);
        let value = match groups.get(&k) {
            Some(acc) => merge(acc.clone(), of(&item)),
            None => of(&item),
        };
        groups.insert(k, value)
    })
}

/// The group an equality join finds for `key`. A NULL key matches nothing.
pub(crate) fn lookup<'a, V>(
    groups: &'a Groups<Option<String>, V>,
    key: &Option<String>,
) -> Option<&'a V> {
    if key.is_some() { groups.get(key) } else { None }
}

/// `SUM` over nullable integers: the sum of the values present, NULL when none is.
pub(crate) fn sum_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `MIN` over nullable values, ignoring NULLs.
pub(crate) fn min_some<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `MAX` over nullable values, ignoring NULLs.
pub(crate) fn max_some<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `MAX` over nullable floats, ignoring NULLs.
pub(crate) fn max_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `COUNT(*) FILTER (WHERE flag)` for one row.
pub(crate) fn count(flag: bool) -> i64 {
    i64::from(flag)
}

/// `COALESCE(value, 0)`.
pub(crate) fn or_zero(value: Option<i64>) -> i64 {
    value.unwrap_or(0)
}
