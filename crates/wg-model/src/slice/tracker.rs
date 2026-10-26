//! [`TrackerSlice`]: the rows a tracker sync reads and writes. The tracker mirrors tasks, so the
//! slice is the task slice, whose `external_items` hold the tracker items.

use crate::slice::TaskSlice;
use crate::write::Write;

/// Loaded rows of the tracker sync: the tasks and their tracker items.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackerSlice {
    pub tasks: TaskSlice,
}

impl TrackerSlice {
    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        TrackerSlice {
            tasks: self.tasks.apply(write),
        }
    }
}
