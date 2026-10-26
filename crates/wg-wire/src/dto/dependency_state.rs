//! `DependencyState`.

use wg_json::Value;

use crate::convention::model;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DependencyState";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 2] = ["satisfied", "unsatisfied"];

/// Whether a task's dependencies are all accepted. `unsatisfied` holds the `TaskBrief` values of
/// the dependencies that are not, in the order the caller lists them.
pub fn from_model(satisfied: bool, unsatisfied: Vec<Value>) -> Value {
    model(
        NAME,
        vec![
            ("satisfied", Value::Bool(satisfied)),
            ("unsatisfied", Value::List(unsatisfied)),
        ],
    )
}
