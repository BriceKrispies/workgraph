//! `ExperimentOut`: an experiment as `experiment_list` and `experiment_get` print it (the Pydantic
//! model of `core/models/schemas.py`).
//!
//! No HTTP route serves experiments, so the OpenAPI snapshot has no component schema for this DTO
//! and it is not listed in [`super::implemented`]. Pydantic datetimes print in the `Z` form.

use serde_json::{Map, Value as Json};
use wg_model::entity::Experiment;
use wg_types::{Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// An experiment row as its DTO.
    pub struct ExperimentOut<'a>("ExperimentOut") value from_model(e: &'a Experiment) {
        id: Id = e.id,
        workspace_id: Id = e.workspace_id,
        slug: &'a str = &e.slug,
        title: &'a str = &e.title,
        question: Option<&'a str> = e.question.as_deref(),
        status: &'a str = &e.status,
        #[serde(serialize_with = "json_map")]
        protocol: &'a Map<String, Json> = &e.protocol,
        #[serde(serialize_with = "model_z")]
        frozen_at: Option<Timestamp> = e.frozen_at,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = e.created_at,
        #[serde(serialize_with = "model_z")]
        updated_at: Timestamp = e.updated_at,
    }
}
