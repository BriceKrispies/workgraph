//! Stack landing: the decisions of `integrations/git/stack_landing.py`. Accepting a verified
//! stack's parent moves each default branch to the verified integration tip, or, when a default
//! branch moved since the stack was cut, rebases the stack and keeps the landing request.
//!
//! Database reads, `git` calls and acceptances stay in the shell; it passes what it read in and
//! carries out the plans returned here. A refusal is a [`Refusal`]; nothing moves when one is
//! returned, and every repository is checked before any branch moves.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wg_types::{ErrorCode, Id, StackItemState, StackStatus, TaskStatus, WgError};

use crate::ejection::returns_to_ready;
use crate::ordered::sorted_by;
use crate::queue::ItemDetail;

/// Task states from which the acceptance service accepts a task (`ACCEPTABLE_TASK_STATUSES`).
pub const ACCEPTABLE_TASK_STATUSES: &[TaskStatus] =
    &[TaskStatus::AwaitingReview, TaskStatus::AwaitingVerification];

/// Jobs that rebuild or move a stack's integration branches (`INTEGRATION_JOB_KINDS`).
pub const INTEGRATION_JOB_KINDS: &[&str] = &["stack_integrate", "stack_verify"];

/// Job states that count as integration work in flight (`INTEGRATION_JOB_STATUSES`).
pub const INTEGRATION_JOB_STATUSES: &[&str] = &["queued", "running"];

/// The mode and reason of a landing the workspace's `stacks.auto_accept` asked for.
pub const AUTO_ACCEPT_MODE: &str = "automated";
pub const AUTO_ACCEPT_REASON: &str = "stack.auto_accept";

/// A job as `_integration_busy` sees it: its kind, status and the `stack_id` of its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRef {
    pub kind: String,
    pub status: String,
    pub stack_id: Option<String>,
}

/// `_integration_busy`: integration work for this stack is queued or running. Another stack's
/// jobs never count.
pub fn integration_busy(jobs: &[JobRef], stack_id: &str) -> bool {
    jobs.iter().any(|job| {
        INTEGRATION_JOB_KINDS.contains(&job.kind.as_str())
            && INTEGRATION_JOB_STATUSES.contains(&job.status.as_str())
            && job.stack_id.as_deref() == Some(stack_id)
    })
}

/// `guard_child`: a child of an active stack leaves it only while no integration work for the
/// stack is in flight.
pub fn guard_child(task_key: &str, stack_id: &str, busy: bool) -> Result<(), WgError> {
    if !busy {
        return Ok(());
    }
    Err(WgError::conflict(format!(
        "{task_key} is in a stack whose integration is queued or running; retry once it settles"
    ))
    .detail("task", task_key)
    .detail("stack_id", stack_id))
}

/// The guard of the rebuild a deferral needs (`_defer` with `guard`).
pub fn guard_defer(stack_id: &str, busy: bool) -> Result<(), WgError> {
    if !busy {
        return Ok(());
    }
    Err(
        WgError::conflict("the stack's integration is queued or running; retry once it settles")
            .detail("stack_id", stack_id),
    )
}

/// One stack item as landing sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LandItem {
    pub key: String,
    pub number: i64,
    pub merge_order: Option<i64>,
    pub state: StackItemState,
    pub task_status: TaskStatus,
    /// Stored detail text.
    pub detail: Option<String>,
}

/// One stack repository as landing sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandRepo {
    pub id: String,
    pub name: String,
    pub default_branch: Option<String>,
    /// The stack's recorded base of this repository.
    pub base_sha: String,
    /// Where the integration branch is now.
    pub integrated_tip: String,
    /// Where the default branch is now.
    pub default_head: String,
}

/// Everything `_land` reads before it moves anything.
#[derive(Debug, Clone, PartialEq)]
pub struct LandState {
    pub parent_key: String,
    pub stack_id: String,
    pub stack_status: StackStatus,
    pub parent_status: TaskStatus,
    pub items: Vec<LandItem>,
    /// Whether the stack has a recorded `stack.verified` event.
    pub verified_event: bool,
    pub repos: Vec<LandRepo>,
    /// The integration tips the stack last verified, by repository name.
    pub tips: BTreeMap<String, String>,
}

/// Why a landing was refused. The kind is the Python exception class, the code names the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    StackNotVerified {
        parent: String,
        status: StackStatus,
    },
    ParentNotAcceptable {
        parent: String,
        status: TaskStatus,
    },
    UnverifiedChildrenWithMerges {
        parent: String,
        unfinished: Vec<String>,
    },
    ChildNotAcceptable {
        task: String,
        status: TaskStatus,
    },
    NoVerificationEvent {
        stack_id: String,
    },
    NoDefaultBranch {
        repository: String,
    },
    IntegrationMoved {
        parent: String,
        repository: String,
    },
}

impl Refusal {
    /// The check that refused.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StackNotVerified { .. } => "stack_not_verified",
            Self::ParentNotAcceptable { .. } => "parent_not_acceptable",
            Self::UnverifiedChildrenWithMerges { .. } => "unverified_children_with_merges",
            Self::ChildNotAcceptable { .. } => "child_not_acceptable",
            Self::NoVerificationEvent { .. } => "no_verification_event",
            Self::NoDefaultBranch { .. } => "no_default_branch",
            Self::IntegrationMoved { .. } => "integration_moved",
        }
    }

    /// The Python exception class (`ConflictError` or `ValidationFailed`).
    pub fn kind(&self) -> &'static str {
        match self.error_code() {
            ErrorCode::ValidationFailed => "ValidationFailed",
            _ => "ConflictError",
        }
    }

    fn error_code(&self) -> ErrorCode {
        match self {
            Self::NoDefaultBranch { .. } => ErrorCode::ValidationFailed,
            _ => ErrorCode::Conflict,
        }
    }

    /// The refusal as the error the service raises, with its message and details.
    pub fn to_error(&self) -> WgError {
        match self {
            Self::StackNotVerified { parent, status } => WgError::new(
                self.error_code(),
                format!("{parent}'s stack is {status}; only a verified stack can land"),
            )
            .detail("parent", parent.as_str())
            .detail("stack_status", status.to_string()),
            Self::ParentNotAcceptable { parent, status } => WgError::new(
                self.error_code(),
                format!("{parent} is {status}; only completed work awaiting review can be accepted"),
            )
            .detail("parent", parent.as_str()),
            Self::UnverifiedChildrenWithMerges { parent, unfinished } => WgError::new(
                self.error_code(),
                format!(
                    "{parent}'s stack has unverified children with merges on the integration branch: {}; verify the stack again or remove those children before accepting the parent",
                    unfinished.join(", ")
                ),
            )
            .detail("parent", parent.as_str())
            .detail("unfinished", unfinished),
            Self::ChildNotAcceptable { task, status } => WgError::new(
                self.error_code(),
                format!("{task} is {status}; a verified child cannot be accepted in that state"),
            )
            .detail("task", task.as_str()),
            Self::NoVerificationEvent { stack_id } => WgError::new(
                self.error_code(),
                format!("stack {stack_id} has no recorded verification"),
            ),
            Self::NoDefaultBranch { repository } => WgError::new(
                self.error_code(),
                format!("repository {repository:?} has no default branch"),
            ),
            Self::IntegrationMoved { parent, repository } => WgError::new(
                self.error_code(),
                format!(
                    "the integration branch of {repository} moved since {parent} was verified; verify the stack again before accepting it"
                ),
            )
            .detail("parent", parent.as_str())
            .detail("repository", repository.as_str()),
        }
    }
}

/// The branch `_land` takes once every precondition holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandDecision {
    /// Every default branch still equals its base: fast-forward and accept `children`.
    Land { children: Vec<String> },
    /// A default branch moved: rebase the stack and keep the request.
    Deferred { children: Vec<String> },
}

fn order_key(item: &&LandItem) -> (bool, i64, i64) {
    (
        item.merge_order.is_none(),
        item.merge_order.unwrap_or(0),
        item.number,
    )
}

/// `_blockers_first`: orders `rows` so each follows the rows it depends on; ties keep the given
/// order. `blockers` maps a task id to the ids it depends on; only ids of `rows` matter. A cycle
/// (which the dependency graph forbids) falls back to the first pending row.
pub fn blockers_first<T>(
    rows: Vec<T>,
    id_of: impl Fn(&T) -> Id,
    blockers: &BTreeMap<Id, BTreeSet<Id>>,
) -> Vec<T> {
    place(rows, &id_of, blockers, BTreeSet::new(), Vec::new())
}

/// Rows with their positions, split into the one placed next and the rest.
type Indexed<T> = (Vec<(usize, T)>, Vec<(usize, T)>);

/// One step of [`blockers_first`]: the first pending row whose blockers are all placed goes next.
fn place<T>(
    pending: Vec<T>,
    id_of: &impl Fn(&T) -> Id,
    blockers: &BTreeMap<Id, BTreeSet<Id>>,
    placed: BTreeSet<Id>,
    ordered: Vec<T>,
) -> Vec<T> {
    let at = pending
        .iter()
        .position(|row| {
            blockers
                .get(&id_of(row))
                .is_none_or(|needed| needed.is_subset(&placed))
        })
        .unwrap_or(0);
    let (taken, rest): Indexed<T> = pending
        .into_iter()
        .enumerate()
        .partition(|(index, _)| *index == at);
    let Some((_, row)) = taken.into_iter().next() else {
        return ordered;
    };
    let placed: BTreeSet<Id> = placed.into_iter().chain([id_of(&row)]).collect();
    let ordered: Vec<T> = ordered.into_iter().chain([row]).collect();
    place(
        rest.into_iter().map(|(_, row)| row).collect(),
        id_of,
        blockers,
        placed,
        ordered,
    )
}

/// The ordered checks of `StackLander._land`, before any git state is changed.
pub fn land_decision(state: &LandState) -> Result<LandDecision, Refusal> {
    let parent = state.parent_key.clone();
    if state.stack_status != StackStatus::Verified {
        return Err(Refusal::StackNotVerified {
            parent,
            status: state.stack_status,
        });
    }
    if !ACCEPTABLE_TASK_STATUSES.contains(&state.parent_status) {
        return Err(Refusal::ParentNotAcceptable {
            parent,
            status: state.parent_status,
        });
    }
    let ordered = sorted_by(state.items.iter(), order_key);
    // An unverified child with merges is on the tip, so landing would land unverified work.
    let partial = sorted_by(
        ordered
            .iter()
            .filter(|item| {
                item.state != StackItemState::Verified
                    && !ItemDetail::load(item.detail.as_deref()).merges.is_empty()
            })
            .map(|item| format!("{} ({})", item.key, item.state)),
        String::clone,
    );
    if !partial.is_empty() {
        return Err(Refusal::UnverifiedChildrenWithMerges {
            parent,
            unfinished: partial,
        });
    }
    let verified: Vec<&LandItem> = ordered
        .iter()
        .copied()
        .filter(|item| item.state == StackItemState::Verified)
        .collect();
    if let Some(item) = verified
        .iter()
        .find(|item| !ACCEPTABLE_TASK_STATUSES.contains(&item.task_status))
    {
        return Err(Refusal::ChildNotAcceptable {
            task: item.key.clone(),
            status: item.task_status,
        });
    }
    let children: Vec<String> = verified.iter().map(|item| item.key.clone()).collect();
    if !state.verified_event {
        return Err(Refusal::NoVerificationEvent {
            stack_id: state.stack_id.clone(),
        });
    }
    let repos = sorted_by(state.repos.iter(), |repo| repo.name.clone());
    let refusal = repos.iter().find_map(|repo| {
        if repo.default_branch.as_deref().is_none_or(str::is_empty) {
            return Some(Refusal::NoDefaultBranch {
                repository: repo.name.clone(),
            });
        }
        (state.tips.get(&repo.name) != Some(&repo.integrated_tip)).then(|| {
            Refusal::IntegrationMoved {
                parent: parent.clone(),
                repository: repo.name.clone(),
            }
        })
    });
    if let Some(refusal) = refusal {
        return Err(refusal);
    }
    if repos.iter().any(|repo| repo.default_head != repo.base_sha) {
        Ok(LandDecision::Deferred { children })
    } else {
        Ok(LandDecision::Land { children })
    }
}

/// A landing request kept on the stack: how and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandRequest {
    pub mode: String,
    pub reason: String,
}

/// `after_verified`: the mode and reason to land with. A stored request wins; otherwise
/// `stacks.auto_accept` lands automatically; otherwise nothing was asked for.
pub fn after_verified_request(
    request: Option<LandRequest>,
    auto_accept: bool,
) -> Option<LandRequest> {
    request.or_else(|| {
        auto_accept.then(|| LandRequest {
            mode: AUTO_ACCEPT_MODE.to_string(),
            reason: AUTO_ACCEPT_REASON.to_string(),
        })
    })
}

/// What `after_verified` reports to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterVerified {
    Landed,
    Deferred,
    Refused,
}

impl AfterVerified {
    /// The word the shell reports (`landed`, `deferred`, `refused`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landed => "landed",
            Self::Deferred => "deferred",
            Self::Refused => "refused",
        }
    }
}

/// The result of a landing attempt: refused, deferred (no acceptance) or landed.
pub fn after_verified_outcome<A>(attempt: &Result<Option<A>, WgError>) -> AfterVerified {
    match attempt {
        Err(_) => AfterVerified::Refused,
        Ok(Some(_)) => AfterVerified::Landed,
        Ok(None) => AfterVerified::Deferred,
    }
}

/// The stored landing request: JSON with sorted keys (`json.dumps(sort_keys=True)`).
pub fn land_request_text(mode: &str, reason: &str, actor: &Value) -> String {
    json!({"actor": actor, "mode": mode, "reason": reason}).to_string()
}

/// `_verified_run`: the passing run and tips of the last `stack.verified` event's data.
pub fn verified_run(
    stack_id: &str,
    event: Option<&Value>,
) -> Result<(Option<String>, BTreeMap<String, String>), Refusal> {
    let data = event.ok_or_else(|| Refusal::NoVerificationEvent {
        stack_id: stack_id.to_string(),
    })?;
    let run = data["run_id"].as_str().map(str::to_string);
    let tips = data["tips"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok((run, tips))
}

/// What the shell read about one repository before `_fast_forward`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfCheck {
    pub repository: String,
    pub default_branch: String,
    pub base_sha: String,
    pub tip: String,
    /// Whether `base_sha` is an ancestor of `tip`.
    pub descends: bool,
    /// The user checkout holding the default branch, and whether it is clean.
    pub holder: Option<(String, bool)>,
}

/// How one default branch moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfStep {
    /// `git merge --ff-only <tip>` in the checkout that holds the branch.
    MergeFfOnly { holder: String },
    /// A compare-and-swap `update-ref refs/heads/<branch> <tip> <base>`; no checkout holds it.
    UpdateRef,
}

/// One planned move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfMove {
    pub repository: String,
    pub default_branch: String,
    pub base_sha: String,
    pub tip: String,
    pub step: FfStep,
}

/// `_fast_forward` (checks): every repository is checked, in order, before any branch moves. A
/// tip that does not descend from the base is a conflict; a holder with uncommitted changes is
/// a validation failure.
pub fn fast_forward_plan(checks: &[FfCheck]) -> Result<Vec<FfMove>, WgError> {
    checks
        .iter()
        .map(|check| {
            if !check.descends {
                return Err(WgError::conflict(format!(
                        "the integration tip {} of {} does not descend from {} at {}",
                        check.tip, check.repository, check.default_branch, check.base_sha
                    ))
                .detail("repository", check.repository.as_str()));
            }
            let step = match &check.holder {
                Some((path, false)) => {
                    return Err(WgError::validation(format!(
                            "{path} has uncommitted changes and has {branch} checked out, so {branch} cannot be fast-forwarded there; commit or stash them, then accept again",
                            branch = check.default_branch
                        ))
                    .detail("repository", check.repository.as_str())
                    .detail("path", path.as_str())
                    .detail("branch", check.default_branch.as_str()));
                }
                Some((path, true)) => FfStep::MergeFfOnly {
                    holder: path.clone(),
                },
                None => FfStep::UpdateRef,
            };
            Ok(FfMove {
                repository: check.repository.clone(),
                default_branch: check.default_branch.clone(),
                base_sha: check.base_sha.clone(),
                tip: check.tip.clone(),
                step,
            })
        })
        .collect()
}

/// The git arguments that make a move.
pub fn move_args(step: &FfMove) -> Vec<String> {
    match &step.step {
        FfStep::MergeFfOnly { .. } => vec!["merge".into(), "--ff-only".into(), step.tip.clone()],
        FfStep::UpdateRef => vec![
            "update-ref".into(),
            format!("refs/heads/{}", step.default_branch),
            step.tip.clone(),
            step.base_sha.clone(),
        ],
    }
}

/// The git arguments that undo a move already made (run without checking, newest move first).
pub fn rollback_args(step: &FfMove) -> Vec<String> {
    match &step.step {
        FfStep::MergeFfOnly { .. } => {
            vec!["reset".into(), "--keep".into(), step.base_sha.clone()]
        }
        FfStep::UpdateRef => vec![
            "update-ref".into(),
            format!("refs/heads/{}", step.default_branch),
            step.base_sha.clone(),
            step.tip.clone(),
        ],
    }
}

/// The moves to undo after a failure, newest first.
pub fn rollback_order(moved: &[FfMove]) -> Vec<&FfMove> {
    moved.iter().rev().collect()
}

/// The refusal when a default branch moved under the landing.
pub fn moved_while_landing() -> WgError {
    WgError::conflict(
        "a default branch moved while the stack was landing; nothing was moved, verify and accept again",
    )
}

/// `rebase_if_stale`: the heads to rebase onto when any default branch no longer equals its base.
/// `None` when every base is current or a repository has no default branch.
pub fn stale_heads(repos: &[LandRepo]) -> Option<BTreeMap<String, String>> {
    let has_default = repos.iter().all(|repo| {
        repo.default_branch
            .as_deref()
            .is_some_and(|b| !b.is_empty())
    });
    let moved = repos.iter().any(|repo| repo.default_head != repo.base_sha);
    (has_default && moved).then(|| {
        repos
            .iter()
            .map(|repo| (repo.name.clone(), repo.default_head.clone()))
            .collect()
    })
}

/// The event type recorded when a deferral keeps the landing request.
pub const LAND_REQUESTED_EVENT: &str = "stack.land_requested";
/// The event type recorded when a stack is rebased before it verified.
pub const REBASED_EVENT: &str = "stack.rebased";
/// The reason of a rebase before verification.
pub const REBASED_REASON: &str = "default branch moved before verification";

/// `_move_bases`: the event data recording the base move; `extra` keys follow the base keys.
pub fn move_bases_event(
    parent_key: &str,
    previous: &BTreeMap<String, String>,
    heads: &BTreeMap<String, String>,
    extra: &Value,
) -> Value {
    let base = json!({
        "parent_key": parent_key,
        "base_shas": previous,
        "new_base_shas": heads,
    });
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => Value::Object(
            base.into_iter()
                .chain(extra.iter().map(|(k, v)| (k.clone(), v.clone())))
                .collect(),
        ),
        (base, _) => base,
    }
}

/// The extra event data of a deferral.
pub fn land_requested_data(mode: &str, reason: &str) -> Value {
    json!({"mode": mode, "reason": reason})
}

/// The extra event data of a stale rebase.
pub fn rebased_data() -> Value {
    json!({"reason": REBASED_REASON})
}

/// `_rebuild`: the base each repository is rebuilt from, `bases` when given, else its recorded base.
pub fn rebuild_base(
    name: &str,
    recorded: &str,
    bases: Option<&BTreeMap<String, String>>,
) -> String {
    bases
        .and_then(|bases| bases.get(name).cloned())
        .unwrap_or_else(|| recorded.to_string())
}

/// `land` failure: the `stack.landing_refused` event data.
pub fn landing_refused_data(mode: &str, reason: &str, message: &str) -> Value {
    json!({"mode": mode, "reason": reason, "error": message})
}

/// The reason a verified child is accepted with.
pub fn child_accept_reason(parent_key: &str) -> String {
    format!("landed with stack {parent_key}")
}

/// The reason the stack becomes `landed`.
pub fn landed_reason(parent_key: &str) -> String {
    format!("{parent_key} accepted")
}

/// The `stack.landed` event data.
pub fn landed_event(
    parent_key: &str,
    tips: &BTreeMap<String, String>,
    run_id: Option<&str>,
    accepted: &[String],
    released: &[String],
    mode: &str,
    reason: &str,
) -> Value {
    json!({
        "parent_key": parent_key,
        "tips": tips,
        "run_id": run_id,
        "tasks": accepted,
        "released": released,
        "mode": mode,
        "reason": reason,
    })
}

/// The `stack.integration_removed` event data for one repository.
pub fn integration_removed_data(repository: &str, branch: &str) -> Value {
    json!({"repository": repository, "branch": branch})
}

/// What releasing an unverified child at landing does to its task.
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiedRelease {
    /// The reason the task returns to `ready`, when its status is returnable.
    pub ready_reason: Option<String>,
    pub comment_body: String,
    pub comment_data: Value,
}

/// `_release_unverified` for one child that is not verified: the task goes back to ready (from a
/// returnable status), a comment says why, and the child leaves the stack.
pub fn release_unverified(
    parent_key: &str,
    task_key: &str,
    item_state: StackItemState,
    task_status: TaskStatus,
    stack_id: &str,
) -> UnverifiedRelease {
    UnverifiedRelease {
        ready_reason: returns_to_ready(task_status)
            .then(|| format!("stack {parent_key} landed without it")),
        comment_body: format!(
            "Stack {parent_key} landed without {task_key}: it was {item_state} and not verified on the integration tip, so it was released from the stack. Next step: resolve it and add it to a new stack with: workgraph stack add {parent_key} {task_key}."
        ),
        comment_data: json!({"stack_item_release": {"state": item_state.to_string(), "stack_id": stack_id}}),
    }
}

/// The items to release when the landing succeeds: every child that is not verified, in order.
pub fn unverified_children(items: &[LandItem]) -> Vec<&LandItem> {
    sorted_by(
        items
            .iter()
            .filter(|item| item.state != StackItemState::Verified),
        order_key,
    )
}

/// One in-stack dependent of a released child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependent {
    pub item_id: String,
    pub task_key: String,
    pub state: StackItemState,
}

/// A dependent sent back to pending, with the detail it is left with.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetDependent {
    pub item_id: String,
    pub task_key: String,
    pub detail: Value,
}

/// `release_child`: what releasing an accepted child from its stack does.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleasePlan {
    /// Dependents that go back to pending (ejected ones stay as they are).
    pub reset: Vec<ResetDependent>,
    /// The `stack.item_released` event data.
    pub event: Value,
    /// Whether the child's commits were on the integration branch, so it must be rebuilt.
    pub rebuild: bool,
}

/// `release_child`: a child accepted on its own leaves the stack. When its commits were on the
/// branch its dependents return to pending and the branch is rebuilt without it.
pub fn release_plan(
    task_key: &str,
    parent_key: &str,
    state: StackItemState,
    dependents: &[Dependent],
) -> ReleasePlan {
    let on_branch = matches!(state, StackItemState::Merged | StackItemState::Verified);
    let reason = format!("dependency {task_key} was accepted on its own");
    let reset = dependents
        .iter()
        .filter(|dependent| on_branch && dependent.state != StackItemState::Ejected)
        .map(|dependent| ResetDependent {
            item_id: dependent.item_id.clone(),
            task_key: dependent.task_key.clone(),
            detail: ItemDetail {
                merges: Default::default(),
                conflicts: Default::default(),
                other: [("reason".to_string(), Value::String(reason.clone()))]
                    .into_iter()
                    .collect(),
            }
            .to_value(),
        })
        .collect();
    ReleasePlan {
        reset,
        event: json!({
            "task_key": task_key,
            "parent_key": parent_key,
            "state": state.to_string(),
            "rebuild": on_branch,
        }),
        rebuild: on_branch,
    }
}

/// `record_parent_rejected`: the `stack.parent_rejected` event data. The stack stays as it is.
pub fn parent_rejected_data(parent_key: &str, reason: &str) -> Value {
    json!({"task_key": parent_key, "reason": reason})
}
