//! Value helpers for the CORE-17 modules: Python dictionary semantics on ordered pair lists and
//! stable sorting, written without mutation (the `Core` tier is StrictPure).

use rpds::RedBlackTreeMap;
use serde_json::{Map, Value};

/// `pairs` with `name` set to `value`: a known name keeps its first position, a new one is
/// appended (Python `dict[name] = value`).
pub(crate) fn dict_with<V: Clone>(
    pairs: Vec<(String, V)>,
    name: &str,
    value: V,
) -> Vec<(String, V)> {
    if pairs.iter().any(|(known, _)| known == name) {
        pairs
            .into_iter()
            .map(|(known, old)| {
                if known == name {
                    (known, value.clone())
                } else {
                    (known, old)
                }
            })
            .collect()
    } else {
        pairs
            .into_iter()
            .chain(std::iter::once((name.to_string(), value)))
            .collect()
    }
}

/// A dictionary built from pairs in order (Python `{k: v for ...}`).
pub(crate) fn dict_from<V: Clone>(
    pairs: impl IntoIterator<Item = (String, V)>,
) -> Vec<(String, V)> {
    pairs.into_iter().fold(Vec::new(), |dict, (name, value)| {
        dict_with(dict, &name, value)
    })
}

/// `items` stably sorted by `key` (Python `sorted(items, key=key)`).
pub(crate) fn sorted_by<T: Clone, K: Ord>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    items
        .into_iter()
        .enumerate()
        .fold(RedBlackTreeMap::new(), |tree, (index, item)| {
            tree.insert((key(&item), index), item)
        })
        .values()
        .cloned()
        .collect()
}

/// `items` sorted, repeats kept (Python `sorted(items)`).
pub(crate) fn sorted<T: Clone + Ord>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    sorted_by(items, T::clone)
}

/// A JSON object with `key` set to `value`, keeping a known key's position.
pub(crate) fn object_with(map: Map<String, Value>, key: &str, value: Value) -> Map<String, Value> {
    dict_with(map.into_iter().collect(), key, value)
        .into_iter()
        .collect()
}

/// A JSON object without `key`, the other keys in order.
pub(crate) fn object_without(map: Map<String, Value>, key: &str) -> Map<String, Value> {
    map.into_iter().filter(|(known, _)| known != key).collect()
}
