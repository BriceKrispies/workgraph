//! Slices: the rows one decision reads, as persistent values (world roots of section 4.6).
//!
//! A slice holds only the rows the shell loaded for the decision's read set. A row that is absent
//! was either not loaded or does not exist; which one is the core's business, because the core
//! decides what to read. Each slice table is a [`RedBlackTreeMapSync`] keyed by the row's primary
//! key, so iteration is in key order and identical on every run (a hash map's order would depend
//! on its hasher).

mod queue;
mod registry;
mod run;
mod stack;
mod task;

pub use queue::QueueSlice;
pub use registry::RegistrySlice;
pub use run::{RunSlice, RunState};
pub use stack::{StackFlow, StackSlice};
pub use task::TaskSlice;

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

/// The lowest value of a key component, so `(parent, lowest)..` starts a range at the first
/// child of `parent`.
trait Lowest {
    fn lowest() -> Self;
}

impl Lowest for Id {
    fn lowest() -> Self {
        Id::nil()
    }
}

impl Lowest for String {
    fn lowest() -> Self {
        String::new()
    }
}

impl Lowest for i64 {
    fn lowest() -> Self {
        i64::MIN
    }
}

/// The rows of a table keyed `(parent, rest)` whose parent is `parent`, in key order.
fn children<K: Ord + Clone + Lowest, V>(
    table: &RedBlackTreeMapSync<(Id, K), V>,
    parent: Id,
) -> impl Iterator<Item = &V> {
    table
        .range((parent, K::lowest())..)
        .take_while(move |((owner, _), _)| *owner == parent)
        .map(|(_, row)| row)
}
