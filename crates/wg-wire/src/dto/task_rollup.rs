//! `TaskRollup`.

use crate::convention::counts_field;
use crate::wire_dto;

wire_dto! {
    /// The progress of a task's descendants. `by_status` holds the counts in the order given.
    pub struct TaskRollup<'a>("TaskRollup")
    value from_model(total_descendants: i64, by_status: &'a [(String, i64)], accepted: i64, progress: f64) {
        total_descendants: i64 = total_descendants,
        #[serde(serialize_with = "counts_field")]
        by_status: &'a [(String, i64)] = by_status,
        accepted: i64 = accepted,
        progress: f64 = progress,
    }
}
