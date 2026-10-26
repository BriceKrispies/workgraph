//! The reply of `classification_request`: the background classification job it queued.

use wg_json::Value;

use crate::convention::{dict, text};

/// The name of the reply dict; it has no OpenAPI component schema.
pub const NAME: &str = "ClassificationRequestOut";

/// The fields in the order `ClassificationService.request` builds them.
pub const FIELDS: [&str; 4] = ["job_id", "task_id", "task_key", "status"];

/// The queued job. `status` is the job's status text, e.g. `queued`.
pub fn from_parts(job_id: &str, task_id: &str, task_key: &str, status: &str) -> Value {
    dict(vec![
        ("job_id", text(job_id)),
        ("task_id", text(task_id)),
        ("task_key", text(task_key)),
        ("status", text(status)),
    ])
}
