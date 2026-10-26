//! `RepositoryBrief`.

use wg_json::Value;
use wg_model::entity::repository::Repository;

use crate::convention::{id, model, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RepositoryBrief";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["id", "name", "access_mode", "health_status"];

/// The short form of a repository, as project summaries list it.
pub fn from_model(repository: &Repository) -> Value {
    model(
        NAME,
        vec![
            ("id", id(repository.id)),
            ("name", text(&repository.name)),
            ("access_mode", text(repository.access_mode.as_str())),
            ("health_status", text(&repository.health_status)),
        ],
    )
}
