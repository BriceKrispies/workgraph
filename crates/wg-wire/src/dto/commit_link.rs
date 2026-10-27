//! `CommitLinkOut`: a commit linked to a task, as `POST /tasks/{ref}/commits` and the
//! `task_link_commit` tool return it.
//!
//! It is a Pydantic model of `integrations/git/service.py`, not of `schemas.py`, and no route
//! declares it as a response model, so the OpenAPI snapshot has no component for it.

use serde_json::{Map, Value as Json};
use wg_model::entity::git::CommitTaskLink;
use wg_types::{Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// A link row with the commit's `sha` and `repository_id` (columns of the commit it points at).
    pub struct CommitLinkOut<'a>("CommitLinkOut")
    value from_model(l: &'a CommitTaskLink, sha: &'a str, repository_id: Id) {
        id: Id = l.id,
        commit_id: Id = l.commit_id,
        sha: &'a str = sha,
        repository_id: Id = repository_id,
        task_id: Id = l.task_id,
        source: &'a str = &l.source,
        confidence: f64 = l.confidence.0,
        #[serde(serialize_with = "json_map")]
        evidence: &'a Map<String, Json> = &l.evidence,
        attempt_id: Option<Id> = l.attempt_id,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = l.created_at,
    }
}
