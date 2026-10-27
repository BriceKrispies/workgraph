//! The reply of `classification_request`: the background classification job it queued. A dict with
//! no OpenAPI component schema.

use crate::wire_dto;

wire_dto! {
    /// The queued job. `status` is the job's status text, e.g. `queued`.
    pub struct ClassificationRequestOut<'a>("ClassificationRequestOut")
    dict from_parts(job_id: &'a str, task_id: &'a str, task_key: &'a str, status: &'a str) {
        job_id: &'a str = job_id,
        task_id: &'a str = task_id,
        task_key: &'a str = task_key,
        status: &'a str = status,
    }
}
