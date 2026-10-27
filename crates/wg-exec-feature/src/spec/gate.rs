//! `StackService.gate`: how a finished attempt is verified, from the stack the task belongs to.

use wg_exec::run::Gate;
use wg_model::TaskSlice;
use wg_model::entity::{Task, VerificationConfig};
use wg_model::slice::StackSlice;
use wg_tasks::deps::INACTIVE_STACK_STATUSES;
use wg_types::{Id, WgError};
use wg_verify::{CheckTier, TaskScope, applicable_checks, checks_for_tier};

/// The facts about a task that decide which verification checks apply to it.
pub fn scope_of(tasks: &TaskSlice, task: &Task) -> TaskScope {
    TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: tasks
            .projects_of(task.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: tasks
            .repositories_of(task.id)
            .map(|row| row.repository_id)
            .collect(),
    }
}

/// Whether the task is an item of a stack that has not landed or been abandoned.
pub fn in_active_stack(stacks: &StackSlice, task_id: Id) -> bool {
    stacks.items.values().any(|item| {
        item.task_id == task_id
            && stacks
                .stack(item.stack_id)
                .is_some_and(|stack| !INACTIVE_STACK_STATUSES.contains(&stack.status))
    })
}

/// `None` outside a stack. Inside one, `Fast` when the task has fast checks (they gate it; the full
/// tier runs on the integration tip) and `Pass` when it has none (it becomes ready without a run).
pub fn gate(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    configs: &[VerificationConfig],
    task_id: Id,
) -> Result<Option<Gate>, WgError> {
    if !in_active_stack(stacks, task_id) {
        return Ok(None);
    }
    let Some(task) = tasks.task(task_id) else {
        return Ok(None);
    };
    let checks = applicable_checks(configs, &scope_of(tasks, task))?;
    Ok(Some(
        if checks_for_tier(&checks, Some(CheckTier::Fast)).is_empty() {
            Gate::Pass
        } else {
            Gate::Fast
        },
    ))
}
