//! `ExperimentOut`: an experiment as `experiment_list` and `experiment_get` print it (the Pydantic
//! model of `core/models/schemas.py`).
//!
//! No HTTP route serves experiments, so the OpenAPI snapshot has no component schema for this DTO
//! and it is not listed in [`super::implemented`]. Pydantic datetimes print in the `Z` form.

use wg_json::Value;
use wg_model::entity::Experiment;

use crate::convention::{
    TimeFormat, id, json_object, model, optional_text, optional_timestamp, text, timestamp,
};

/// The Python class of this DTO.
pub const NAME: &str = "ExperimentOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "workspace_id",
    "slug",
    "title",
    "question",
    "status",
    "protocol",
    "frozen_at",
    "created_at",
    "updated_at",
];

/// An experiment row as its DTO.
pub fn from_model(experiment: &Experiment) -> Value {
    model(
        NAME,
        vec![
            ("id", id(experiment.id)),
            ("workspace_id", id(experiment.workspace_id)),
            ("slug", text(&experiment.slug)),
            ("title", text(&experiment.title)),
            ("question", optional_text(experiment.question.as_deref())),
            ("status", text(&experiment.status)),
            ("protocol", json_object(&experiment.protocol)),
            (
                "frozen_at",
                optional_timestamp(experiment.frozen_at, TimeFormat::ModelZ),
            ),
            (
                "created_at",
                timestamp(experiment.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(experiment.updated_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
