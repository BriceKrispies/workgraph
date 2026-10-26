//! `CheckoutOut`.

use wg_json::Value;
use wg_model::entity::repository::RepositoryCheckout;

use wg_wire::convention::{TimeFormat, id, model, optional_timestamp, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "CheckoutOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 7] = [
    "id",
    "path",
    "kind",
    "is_primary",
    "available",
    "access_mode",
    "last_seen_at",
];

/// One checkout of a repository. `access_mode` is the checkout's own override, `null` when unset.
pub fn from_model(checkout: &RepositoryCheckout) -> Value {
    model(
        NAME,
        vec![
            ("id", id(checkout.id)),
            ("path", text(&checkout.path)),
            ("kind", text(&checkout.kind)),
            ("is_primary", Value::Bool(checkout.is_primary)),
            ("available", Value::Bool(checkout.available)),
            (
                "access_mode",
                checkout.access_mode.as_deref().map_or(Value::Null, text),
            ),
            (
                "last_seen_at",
                optional_timestamp(checkout.last_seen_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
