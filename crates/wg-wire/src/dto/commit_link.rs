//! `CommitLinkOut`: a commit linked to a task, as `POST /tasks/{ref}/commits` and the
//! `task_link_commit` tool return it.
//!
//! It is a Pydantic model of `integrations/git/service.py`, not of `schemas.py`, and no route
//! declares it as a response model, so the OpenAPI snapshot has no component for it.

use wg_json::Value;
use wg_model::entity::git::CommitTaskLink;
use wg_types::Id;

use crate::convention::{TimeFormat, id, json_object, model, optional_id, text, timestamp};

/// The Python class name.
pub const NAME: &str = "CommitLinkOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "commit_id",
    "sha",
    "repository_id",
    "task_id",
    "source",
    "confidence",
    "evidence",
    "attempt_id",
    "created_at",
];

/// A link row with the commit's `sha` and `repository_id` (columns of the commit it points at).
pub fn from_model(link: &CommitTaskLink, sha: &str, repository_id: Id) -> Value {
    model(
        NAME,
        vec![
            ("id", id(link.id)),
            ("commit_id", id(link.commit_id)),
            ("sha", text(sha)),
            ("repository_id", id(repository_id)),
            ("task_id", id(link.task_id)),
            ("source", text(&link.source)),
            ("confidence", Value::Float(link.confidence.0)),
            ("evidence", json_object(&link.evidence)),
            ("attempt_id", optional_id(link.attempt_id)),
            ("created_at", timestamp(link.created_at, TimeFormat::ModelZ)),
        ],
    )
}
