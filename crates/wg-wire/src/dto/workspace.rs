//! `WorkspaceOut`.

use wg_json::Value;
use wg_model::entity::workspace::Workspace;

use crate::convention::{TimeFormat, id, json_object, model, optional_timestamp, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "WorkspaceOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 9] = [
    "id",
    "slug",
    "name",
    "description",
    "key_prefix",
    "settings",
    "created_at",
    "updated_at",
    "archived_at",
];

/// The workspace as the API returns it.
pub fn from_model(workspace: &Workspace) -> Value {
    model(
        NAME,
        vec![
            ("id", id(workspace.id)),
            ("slug", text(&workspace.slug)),
            ("name", text(&workspace.name)),
            ("description", text(&workspace.description)),
            ("key_prefix", text(&workspace.key_prefix)),
            ("settings", json_object(&workspace.settings)),
            (
                "created_at",
                timestamp(workspace.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(workspace.updated_at, TimeFormat::ModelZ),
            ),
            (
                "archived_at",
                optional_timestamp(workspace.archived_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
