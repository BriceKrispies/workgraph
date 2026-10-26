//! `StackRepositoryOut`.

use wg_json::Value;
use wg_model::entity::stack::StackRepository;

use crate::convention::{id, model, optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "StackRepositoryOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 5] = [
    "repository_id",
    "repository_name",
    "base_sha",
    "integration_branch",
    "integration_tip",
];

/// One repository of a stack. `repository_name` comes from the `repositories` row, which the
/// caller joins in. `integration_tip` is always `null`: the Python `stack_out` never sets it, and
/// the DTO prints what `model_dump` prints.
pub fn from_model(row: &StackRepository, repository_name: &str) -> Value {
    with_tip(row, repository_name, None)
}

/// One repository of a stack with the tip of its prepared integration worktree, as the stacks
/// service attaches it to the `get` and `list` replies (`StackService._with_tips`); `None` while
/// the worktree does not exist.
pub fn with_tip(row: &StackRepository, repository_name: &str, tip: Option<&str>) -> Value {
    model(
        NAME,
        vec![
            ("repository_id", id(row.repository_id)),
            ("repository_name", text(repository_name)),
            ("base_sha", text(&row.base_sha)),
            ("integration_branch", text(&row.integration_branch)),
            ("integration_tip", optional_text(tip)),
        ],
    )
}
