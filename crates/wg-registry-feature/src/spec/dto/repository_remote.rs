//! `RemoteOut`.

use wg_json::Value;
use wg_model::entity::repository::RepositoryRemote;

use wg_wire::convention::{model, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RemoteOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 3] = ["name", "url", "normalized_url"];

/// One remote of a repository.
pub fn from_model(remote: &RepositoryRemote) -> Value {
    model(
        NAME,
        vec![
            ("name", text(&remote.name)),
            ("url", text(&remote.url)),
            ("normalized_url", text(&remote.normalized_url)),
        ],
    )
}
