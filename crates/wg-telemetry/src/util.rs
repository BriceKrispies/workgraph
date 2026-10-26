//! Order-preserving grouping shared by the modules. A Python `dict` keeps the position of a key's first
//! insertion and the value of its last; these helpers do the same with persistent collections, so no
//! binding in this crate needs `mut`.

use rpds::{RedBlackTreeMap, Vector};

/// Groups `(key, value)` pairs by key. Each key appears once, at the position of its first pair, and its
/// value is `combine(previous, new)` folded over the pairs in order.
pub fn group_in_order<K, V, I, F>(pairs: I, combine: F) -> Vec<(K, V)>
where
    K: Ord + Clone,
    V: Clone,
    I: IntoIterator<Item = (K, V)>,
    F: Fn(&V, V) -> V,
{
    let (values, order) = pairs.into_iter().fold(
        (RedBlackTreeMap::<K, V>::new(), Vector::<K>::new()),
        |(values, order), (key, value)| match values.get(&key) {
            Some(previous) => (values.insert(key, combine(previous, value)), order),
            None => (values.insert(key.clone(), value), order.push_back(key)),
        },
    );
    order
        .iter()
        .filter_map(|key| values.get(key).map(|value| (key.clone(), value.clone())))
        .collect()
}

/// The distinct items in first-seen order.
pub fn unique_in_order<T: Ord + Clone>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    group_in_order(items.into_iter().map(|item| (item, ())), |_, ()| ())
        .into_iter()
        .map(|(item, ())| item)
        .collect()
}

/// `collections.Counter` over `items`: each distinct item with its count, in first-seen order.
pub fn count_in_order<T: Ord + Clone>(items: impl IntoIterator<Item = T>) -> Vec<(T, u64)> {
    group_in_order(items.into_iter().map(|item| (item, 1_u64)), |count, one| {
        count + one
    })
}

/// The first element holding the largest count, as `Counter.most_common(1)` picks it: ties go to the
/// earlier element.
pub fn most_common<T: Clone>(counts: &[(T, u64)]) -> Option<T> {
    counts
        .iter()
        .fold(None, |best: Option<&(T, u64)>, entry| match best {
            Some(current) if current.1 >= entry.1 => Some(current),
            _ => Some(entry),
        })
        .map(|(item, _)| item.clone())
}
