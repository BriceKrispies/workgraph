//! `ProjectOut`.

use wg_json::Value;
use wg_model::entity::project::Project;
use wg_types::Id;

use crate::convention::{TimeFormat, id, ids, json_object, model, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "ProjectOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "workspace_id",
    "slug",
    "name",
    "description",
    "status",
    "settings",
    "repository_ids",
    "created_at",
    "updated_at",
];

/// The project as the API returns it; `repository_ids` come from `project_repositories`, in the
/// order the caller lists them.
pub fn from_model(project: &Project, repository_ids: impl IntoIterator<Item = Id>) -> Value {
    model(
        NAME,
        vec![
            ("id", id(project.id)),
            ("workspace_id", id(project.workspace_id)),
            ("slug", text(&project.slug)),
            ("name", text(&project.name)),
            ("description", text(&project.description)),
            ("status", text(&project.status)),
            ("settings", json_object(&project.settings)),
            ("repository_ids", ids(repository_ids)),
            (
                "created_at",
                timestamp(project.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(project.updated_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
