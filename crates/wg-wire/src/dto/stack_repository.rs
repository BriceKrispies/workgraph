//! `StackRepositoryOut`.

use wg_json::Value;
use wg_model::entity::stack::StackRepository;
use wg_types::Id;

use crate::wire_dto;

wire_dto! {
    /// One repository of a stack with the tip of its prepared integration worktree, as the stacks
    /// service attaches it to the `get` and `list` replies (`StackService._with_tips`); `None`
    /// while the worktree does not exist. `repository_name` comes from the `repositories` row,
    /// which the caller joins in.
    pub struct StackRepositoryOut<'a>("StackRepositoryOut")
    value with_tip(row: &'a StackRepository, repository_name: &'a str, tip: Option<&'a str>) {
        repository_id: Id = row.repository_id,
        repository_name: &'a str = repository_name,
        base_sha: &'a str = &row.base_sha,
        integration_branch: &'a str = &row.integration_branch,
        integration_tip: Option<&'a str> = tip,
    }
}

/// One repository of a stack. `integration_tip` is always `null`: the Python `stack_out` never
/// sets it, and the DTO prints what `model_dump` prints.
pub fn from_model(row: &StackRepository, repository_name: &str) -> Value {
    with_tip(row, repository_name, None)
}
