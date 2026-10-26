//! `RepositoryOut`.

use wg_json::Value;
use wg_model::entity::repository::{Repository, RepositoryCheckout, RepositoryRemote};
use wg_types::Id;

use crate::convention::{
    TimeFormat, id, ids, json_object, model, optional_timestamp, text, timestamp,
};
use crate::dto::{repository_checkout, repository_remote};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RepositoryOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 20] = [
    "id",
    "workspace_id",
    "name",
    "description",
    "identity_fingerprint",
    "default_branch",
    "tracked_branches",
    "access_mode",
    "health_status",
    "health_detail",
    "health_checked_at",
    "is_shallow",
    "last_scanned_at",
    "archived_at",
    "remotes",
    "checkouts",
    "project_ids",
    "depends_on",
    "created_at",
    "updated_at",
];

/// The repository as the API returns it. The caller passes the repository's own remotes and
/// checkouts, the ids of the projects it is in (`project_repositories`) and the ids of the
/// repositories it declares it depends on (`repository_dependencies`, sorted by the caller).
pub fn from_model(
    repository: &Repository,
    remotes: &[RepositoryRemote],
    checkouts: &[RepositoryCheckout],
    project_ids: impl IntoIterator<Item = Id>,
    depends_on: impl IntoIterator<Item = Id>,
) -> Value {
    model(
        NAME,
        vec![
            ("id", id(repository.id)),
            ("workspace_id", id(repository.workspace_id)),
            ("name", text(&repository.name)),
            ("description", text(&repository.description)),
            (
                "identity_fingerprint",
                repository
                    .identity_fingerprint
                    .as_deref()
                    .map_or(Value::Null, text),
            ),
            (
                "default_branch",
                repository
                    .default_branch
                    .as_deref()
                    .map_or(Value::Null, text),
            ),
            (
                "tracked_branches",
                Value::List(
                    repository
                        .tracked_branches
                        .iter()
                        .map(|b| text(b))
                        .collect(),
                ),
            ),
            ("access_mode", text(repository.access_mode.as_str())),
            ("health_status", text(&repository.health_status)),
            ("health_detail", json_object(&repository.health_detail)),
            (
                "health_checked_at",
                optional_timestamp(repository.health_checked_at, TimeFormat::ModelZ),
            ),
            ("is_shallow", Value::Bool(repository.is_shallow)),
            (
                "last_scanned_at",
                optional_timestamp(repository.last_scanned_at, TimeFormat::ModelZ),
            ),
            (
                "archived_at",
                optional_timestamp(repository.archived_at, TimeFormat::ModelZ),
            ),
            (
                "remotes",
                Value::List(remotes.iter().map(repository_remote::from_model).collect()),
            ),
            (
                "checkouts",
                Value::List(
                    checkouts
                        .iter()
                        .map(repository_checkout::from_model)
                        .collect(),
                ),
            ),
            ("project_ids", ids(project_ids)),
            ("depends_on", ids(depends_on)),
            (
                "created_at",
                timestamp(repository.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(repository.updated_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
