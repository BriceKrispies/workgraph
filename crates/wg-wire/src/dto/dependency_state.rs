//! `DependencyState`.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// Whether a task's dependencies are all accepted. `unsatisfied` holds the `TaskBrief` values of
    /// the dependencies that are not, in the order the caller lists them.
    pub struct DependencyState("DependencyState")
    value from_model(satisfied: bool, unsatisfied: Vec<Value>) {
        satisfied: bool = satisfied,
        unsatisfied: Vec<Value> = unsatisfied,
    }
}
