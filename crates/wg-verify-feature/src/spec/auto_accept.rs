//! Pure: the automatic acceptance that follows a passing full-tier run
//! (`VerificationService._auto_accept`).
//!
//! It runs in its own transaction after the run is finalised. Stack children are accepted by the
//! stack landing and a stack parent lands its stack, so neither is accepted here; nor is a task
//! a tracker closes, or one whose change is in a repository that lands through stacks.

use wg_model::Decision;
use wg_model::entity::VerificationRun;
use wg_model::stamp::Stamp;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::accept::{AcceptFacts, Run, accept_in, active_item};
use wg_types::{Id, Permission, TaskStatus, WgError};
use wg_verify::{ApplicableCheck, AutoAcceptFacts, auto_accept_skip, ran_fast_tier_only};

/// The mode and reason an automatic acceptance records.
const MODE: &str = "automated";
const REASON: &str = "verification.auto_accept";

/// What the automatic acceptance reads.
#[derive(Debug, Clone)]
pub struct AutoAcceptWorld {
    pub world: TaskWorld,
    /// The run as finalised.
    pub run: VerificationRun,
    /// The newest run of the task.
    pub latest_run: Option<Id>,
    /// `verification.auto_accept` in the effective configuration.
    pub enabled: bool,
    /// A repository of the run's manifest has taken part in a stack.
    pub lands_through_stacks: bool,
    /// The run's manifest and the task's latest succeeded attempt, which the acceptance keeps.
    pub facts: AcceptFacts,
}

/// `verification.auto_accept` of an effective configuration (on when the configuration omits it).
pub fn enabled_in(config: &serde_json::Map<String, serde_json::Value>) -> bool {
    config
        .get("verification")
        .and_then(|section| section.get("auto_accept"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

/// Accepts the task when every guard allows it. The reply says whether it did.
pub fn auto_accept(
    loaded: &AutoAcceptWorld,
    stamp: &Stamp,
    task_id: Id,
    checks: &[ApplicableCheck],
) -> Result<Decision<bool>, WgError> {
    let tasks = &loaded.world.tasks;
    let task = tasks
        .task(task_id)
        .ok_or_else(|| WgError::not_found(format!("task {task_id} not found")))?;
    let snapshot: Vec<_> = loaded.run.config_snapshot.iter().cloned().collect();
    let facts = AutoAcceptFacts {
        holds_task_accept: stamp.has(Permission::TaskAccept),
        auto_accept_enabled: loaded.enabled,
        any_required_check: checks.iter().any(|check| check.spec.required),
        ran_fast_tier_only: ran_fast_tier_only(checks, &snapshot),
        tracked: tasks.external_item_of(task.id).is_some(),
        awaiting_review: task.status == TaskStatus::AwaitingReview,
        active_stack_item: active_item(&loaded.world.stacks, task.id).is_some(),
        active_stack_parent: loaded
            .world
            .stacks
            .active_stack_of_parent(task.id)
            .is_some(),
        lands_through_stacks: loaded.lands_through_stacks,
        is_latest_run: loaded.latest_run == Some(loaded.run.id),
    };
    if auto_accept_skip(&facts).is_some() {
        return Ok(Decision::new(false));
    }
    let (accepted, _) = accept_in(
        &Run::start(&loaded.world, stamp),
        stamp,
        task.id,
        MODE,
        REASON,
        &loaded.facts,
    )?;
    Ok(accepted.applied.decision.with_reply(true))
}
