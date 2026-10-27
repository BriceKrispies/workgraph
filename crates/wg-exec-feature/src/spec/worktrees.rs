//! The pure half of the worktree service (WG-3773): crash recovery of managed worktrees as
//! `WorktreeManager.recover` in `integrations/git/worktrees.py` decides it, the stack a worktree
//! path belongs to, and the outputs the worktree commands print.
//!
//! The shell (`service::worktrees`) reads the rows, the attempts, tasks and stacks
//! they belong to, checks the checkouts on disk and lists the worktree directory. Everything here
//! takes those facts as values.

use std::collections::BTreeSet;
use std::path::Path;

use rpds::VectorSync;
use wg_exec::manifest::{
    ExecutionManifest, ManifestEntry, PlanError, change_set_plan, expected_changes,
};
use wg_exec::recovery::{RecoveryAction, WorktreeFacts, recovery_action};
use wg_git::GitError;
use wg_json::Value;
use wg_model::decision::Decision;
use wg_model::entity::{
    AgentAttempt, ChangeSet, ChangeSetItem, JsonObject, ManagedWorktree, ResourceLease,
    RevisionManifest, Task, VerificationConfig,
};
use wg_model::event::{
    AttemptWorkspacePrepared, EventDraft, EventPayload, WorktreeCreated, WorktreeRecovered,
    WorktreeRemoved,
};
use wg_model::stamp::Stamp;
use wg_model::write::{RowKey, Write};
use wg_registry::boundary::is_within;
use wg_storecodec::run_sql::WorktreeWithRepository;
use wg_types::{
    AttemptStatus, ChangeSetItemStatus, ChangeSetStatus, ErrorCode, Id, Timestamp, WgError,
    WorktreeStatus,
};
use wg_verify::{ApplicableCheck, CheckTier, TaskScope, applicable_checks, checks_for_tier};
use wg_wire::convention::{
    TimeFormat, dict, id, optional_id, optional_text, optional_timestamp, text, texts, timestamp,
};

/// The `last_error` of a row whose checkout is missing (`recover`).
pub const MISSING_ON_DISK: &str = "worktree missing on disk during recovery";

/// What recovery reads about one managed worktree row that is preparing or active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRow {
    pub row: ManagedWorktree,
    /// The checkout exists on disk and is a Git repository.
    pub exists_on_disk: bool,
    /// The owning attempt's status, when the row has an attempt that exists.
    pub attempt_status: Option<AttemptStatus>,
    /// The owning attempt's task was accepted or cancelled, or is archived.
    pub task_closed: bool,
    /// The stack the path belongs to landed or was abandoned.
    pub stack_ended: bool,
}

/// A directory under an attempt directory: the path as Python prints it, and its comparison key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub key: String,
}

/// Everything recovery decides from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryInput {
    /// The rows whose status is preparing or active.
    pub rows: Vec<RecoveryRow>,
    /// The key of every managed worktree path, whatever its status.
    pub known: BTreeSet<String>,
    /// The directories under the attempt directories, in scan order.
    pub candidates: Vec<Candidate>,
}

/// What recovery did, by path (the `fixed` dict of `WorktreeManager.recover`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    pub activated: Vec<String>,
    pub released: Vec<String>,
    pub orphaned: Vec<String>,
    pub unknown_dirs: Vec<String>,
}

/// The comparison key of a path: Python's path equality, which ignores case on Windows.
pub fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

/// The stack id of a stack worktree (`<worktrees>/stacks/<stack id>/<directory>`), else `None`.
/// Both paths are resolved already (`_stack_of`).
pub fn stack_of(path: &str, stacks_root: &str) -> Option<String> {
    if !is_within(path, stacks_root) {
        return None;
    }
    let parent = Path::new(path).parent()?;
    let grandparent = parent.parent()?;
    if path_key(&grandparent.to_string_lossy()) != path_key(stacks_root) {
        return None;
    }
    parent.file_name()?.to_str().map(str::to_owned)
}

/// Decides recovery: each open row becomes orphaned, released, activated or left alone, and the
/// unrecorded directories are reported. A `worktree.recovered` event follows when any row changed.
pub fn recovery_decision(input: &RecoveryInput, stamp: &Stamp) -> Decision<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let mut writes: Vec<Write> = Vec::new();
    for item in &input.rows {
        let facts = WorktreeFacts {
            status: item.row.status,
            exists_on_disk: item.exists_on_disk,
            attempt_status: item.attempt_status,
            task_closed: item.task_closed,
            stack_ended: item.stack_ended,
        };
        let path = item.row.path.clone();
        let change = |status: WorktreeStatus, last_error: Option<String>| {
            Write::update(ManagedWorktree {
                status,
                last_error,
                ..item.row.clone()
            })
        };
        match recovery_action(facts) {
            RecoveryAction::Orphan => {
                report.orphaned.push(path);
                writes.push(change(
                    WorktreeStatus::Orphaned,
                    Some(MISSING_ON_DISK.to_owned()),
                ));
            }
            RecoveryAction::Release => {
                report.released.push(path);
                writes.push(change(
                    WorktreeStatus::Released,
                    item.row.last_error.clone(),
                ));
            }
            RecoveryAction::Activate => {
                report.activated.push(path);
                writes.push(change(WorktreeStatus::Active, item.row.last_error.clone()));
            }
            RecoveryAction::Keep => {}
        }
    }
    report.unknown_dirs = input
        .candidates
        .iter()
        .filter(|candidate| !input.known.contains(&candidate.key))
        .map(|candidate| candidate.path.clone())
        .collect();
    let changed = !writes.is_empty();
    let counts = WorktreeRecovered {
        activated: len(&report.activated),
        orphaned: len(&report.orphaned),
        released: len(&report.released),
        unknown_dirs: len(&report.unknown_dirs),
    };
    let decision = writes
        .into_iter()
        .fold(Decision::new(()), |decision, write| decision.write(write));
    let decision = if changed {
        let (event_id, _) = stamp.ids().next();
        decision.event(EventDraft::new(
            stamp,
            event_id,
            None,
            Id::nil(),
            EventPayload::WorktreeRecovered(counts),
        ))
    } else {
        decision
    };
    decision.with_reply(report)
}

fn len(paths: &[String]) -> i64 {
    i64::try_from(paths.len()).unwrap_or(i64::MAX)
}

/// The report of `worker recover`'s `worktrees` entry: the four path lists, in Python's key order.
pub fn recovery_report_out(report: &RecoveryReport) -> Value {
    dict(vec![
        (
            "activated",
            texts(report.activated.iter().map(String::as_str)),
        ),
        (
            "released",
            texts(report.released.iter().map(String::as_str)),
        ),
        (
            "orphaned",
            texts(report.orphaned.iter().map(String::as_str)),
        ),
        (
            "unknown_dirs",
            texts(report.unknown_dirs.iter().map(String::as_str)),
        ),
    ])
}

/// `WorktreeManager.list`: one dict per managed worktree, in the order the rows came (newest
/// first), with the name of its repository. Datetimes print as Python's `isoformat()`.
pub fn worktree_list_out(rows: &[WorktreeWithRepository]) -> Value {
    Value::List(rows.iter().map(worktree_item).collect())
}

fn worktree_item(row: &WorktreeWithRepository) -> Value {
    let worktree = &row.worktree;
    dict(vec![
        ("id", id(worktree.id)),
        ("repository", text(&row.repository_name)),
        ("repository_id", id(worktree.repository_id)),
        ("attempt_id", optional_id(worktree.attempt_id)),
        ("path", text(&worktree.path)),
        ("branch", text(&worktree.branch)),
        ("base_sha", text(&worktree.base_sha)),
        ("status", text(worktree.status.as_str())),
        ("last_error", optional_text(worktree.last_error.as_deref())),
        (
            "created_at",
            timestamp(worktree.created_at, TimeFormat::Isoformat),
        ),
        (
            "removed_at",
            optional_timestamp(worktree.removed_at, TimeFormat::Isoformat),
        ),
    ])
}

/// A managed worktree `release` targets: its id, its path and the checkout it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTarget {
    pub id: Id,
    pub path: String,
    pub source_checkout_id: Id,
}

/// Decides `release`: the leases the attempt holds are deleted, and each of its worktree rows that
/// is not released yet becomes released. The reply is every row the attempt has, as targets for
/// the removal that follows when asked for.
pub fn release_decision(
    leases: &[ResourceLease],
    rows: &[ManagedWorktree],
) -> Decision<Vec<ReleaseTarget>> {
    let released = leases.iter().fold(Decision::new(()), |decision, lease| {
        decision.write(Write::delete(RowKey::ResourceLease(
            lease.resource_key.clone(),
        )))
    });
    let released = rows
        .iter()
        .filter(|row| row.status != WorktreeStatus::Released)
        .fold(released, |decision, row| {
            decision.write(Write::update(ManagedWorktree {
                status: WorktreeStatus::Released,
                ..row.clone()
            }))
        });
    let targets = rows
        .iter()
        .map(|row| ReleaseTarget {
            id: row.id,
            path: row.path.clone(),
            source_checkout_id: row.source_checkout_id,
        })
        .collect();
    released.with_reply(targets)
}

/// Decides a removal that went through: the row is removed at the stamp's time and the
/// `worktree.removed` event names its branch, which is kept.
pub fn removed_decision(row: &ManagedWorktree, stamp: &Stamp) -> Decision<()> {
    let (event_id, _) = stamp.ids().next();
    Decision::new(())
        .write(Write::update(ManagedWorktree {
            status: WorktreeStatus::Removed,
            removed_at: Some(stamp.at),
            updated_at: stamp.at,
            ..row.clone()
        }))
        .event(EventDraft {
            attempt_id: row.attempt_id,
            ..EventDraft::new(
                stamp,
                event_id,
                Some(row.workspace_id),
                row.id,
                EventPayload::WorktreeRemoved(WorktreeRemoved {
                    path: row.path.clone(),
                    branch_kept: row.branch.clone(),
                }),
            )
        })
}

// ------------------------------------------------------------------ preparation (WG-3773)

/// How long a lease lasts unless the caller says otherwise (`acquire_lease`'s default ttl), seconds.
pub const LEASE_TTL_SECONDS: i64 = 6 * 3600;

/// `execution.isolation` from the configuration, `worktree` when unset.
pub fn configured_isolation(config: &serde_json::Map<String, serde_json::Value>) -> String {
    config
        .get("execution")
        .and_then(|execution| execution.get("isolation"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("worktree")
        .to_owned()
}

/// One repository `prepare` sets up: the access the task asked for, and the ref the attempt starts
/// from when the caller chose one (`RepoRequest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareRequest {
    pub repository_id: Id,
    pub access: String,
    pub target_ref: Option<String>,
}

/// `acquire_lease`: `existing` is the lease row of `resource_key`, if there is one. The holder of
/// a lease keeps it with a fresh expiry; another holder of a live lease makes the resource busy;
/// an expired lease, or one without an expiry that is not the holder's, is taken over.
pub fn acquire_lease_decision(
    existing: Option<&ResourceLease>,
    resource_key: &str,
    holder: &str,
    purpose: &str,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let expires_at = lease_expiry(stamp.at)?;
    let fresh = ResourceLease {
        resource_key: resource_key.to_owned(),
        holder: holder.to_owned(),
        purpose: purpose.to_owned(),
        acquired_at: stamp.at,
        expires_at: Some(expires_at),
    };
    match existing {
        None => Ok(Decision::new(()).write(Write::insert(fresh))),
        Some(current) if current.holder == holder => {
            Ok(Decision::new(()).write(Write::update(ResourceLease {
                expires_at: Some(expires_at),
                ..current.clone()
            })))
        }
        Some(current) if current.expires_at.is_none_or(|expiry| expiry > stamp.at) => {
            Err(WgError::new(
                ErrorCode::ResourceBusy,
                format!("resource {resource_key} is leased by {}", current.holder),
            )
            .with_detail("resource", serde_json::json!(resource_key))
            .with_detail("holder", serde_json::json!(current.holder))
            .with_detail("purpose", serde_json::json!(current.purpose)))
        }
        Some(_) => Ok(Decision::new(()).write(Write::update(fresh))),
    }
}

/// The expiry of a lease taken at `now`.
fn lease_expiry(now: Timestamp) -> Result<Timestamp, WgError> {
    Timestamp::from_offset_datetime(
        now.to_offset_datetime() + time::Duration::seconds(LEASE_TTL_SECONDS),
    )
}

/// The `[<check>]` entries an attempt's manifest carries: every applicable check in full form
/// (`verification`), and the fast-tier checks the agent runs itself (`agent_checks`).
pub fn prepare_checks(
    applicable: &[ApplicableCheck],
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let verification = applicable
        .iter()
        .map(|check| serde_json::Value::Object(check.spec.to_json()))
        .collect();
    let agent_checks = checks_for_tier(applicable, Some(CheckTier::Fast))
        .into_iter()
        .map(|check| {
            serde_json::json!({
                "name": check.spec.name,
                "command": check.spec.command,
                "repository": check.spec.repository,
            })
        })
        .collect();
    (verification, agent_checks)
}

/// A managed worktree row about to be created for `prepare` (`ManagedWorktree` in `_prepare_repo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDraft<'a> {
    pub workspace_id: Id,
    pub repository_id: Id,
    pub source_checkout_id: Id,
    pub attempt_id: Id,
    pub path: &'a str,
    /// The branch the worktree is created on, or `None` for a detached snapshot.
    pub branch: Option<&'a str>,
    pub base_sha: &'a str,
}

/// Inserts the row of a worktree `prepare` is about to create, in `preparing` status. The reply
/// is its id.
pub fn preparing_decision(draft: &WorktreeDraft<'_>, stamp: &Stamp) -> Decision<Id> {
    let (id, _) = stamp.ids().next();
    Decision::new(id).write(Write::insert(ManagedWorktree {
        id,
        workspace_id: draft.workspace_id,
        repository_id: draft.repository_id,
        source_checkout_id: draft.source_checkout_id,
        attempt_id: Some(draft.attempt_id),
        path: draft.path.to_owned(),
        branch: draft.branch.unwrap_or("(detached)").to_owned(),
        base_sha: draft.base_sha.to_owned(),
        status: WorktreeStatus::Preparing,
        last_error: None,
        created_at: stamp.at,
        updated_at: stamp.at,
        removed_at: None,
    }))
}

/// The worktree was created: its row becomes active and `worktree.created` names it. `repository`
/// is the repository's name; `branch` and `base` are `None` for a detached snapshot and a base that
/// no stack chose.
pub fn activated_decision(
    row: &ManagedWorktree,
    repository: &str,
    branch: Option<&str>,
    base: Option<serde_json::Value>,
    stamp: &Stamp,
) -> Decision<()> {
    let (event_id, _) = stamp.ids().next();
    Decision::new(())
        .write(Write::update(ManagedWorktree {
            status: WorktreeStatus::Active,
            ..row.clone()
        }))
        .event(EventDraft {
            attempt_id: row.attempt_id,
            ..EventDraft::new(
                stamp,
                event_id,
                Some(row.workspace_id),
                row.id,
                EventPayload::WorktreeCreated(WorktreeCreated {
                    purpose: None,
                    stack_id: None,
                    repository: repository.to_owned(),
                    path: row.path.clone(),
                    branch: branch.map(str::to_owned),
                    base_sha: row.base_sha.clone(),
                    base,
                }),
            )
        })
}

/// The worktree could not be created: its row is orphaned with git's refusal as its error.
pub fn orphaned_decision(row: &ManagedWorktree, message: &str) -> Decision<()> {
    Decision::new(()).write(Write::update(ManagedWorktree {
        status: WorktreeStatus::Orphaned,
        last_error: Some(message.to_owned()),
        ..row.clone()
    }))
}

/// The facts `prepare`'s final decision is made from: the attempt and its task, the planned
/// repository entries, the caller's expected changes, the isolation, the workspace directory, and
/// the applicable configurations with the task's scope.
#[derive(Debug, Clone, Copy)]
pub struct PrepareFacts<'a> {
    pub attempt: &'a AgentAttempt,
    pub task: &'a Task,
    pub entries: &'a [ManifestEntry],
    pub explicit_expected: &'a [String],
    pub isolation: &'a str,
    pub workspace_dir: &'a str,
    pub configs: &'a [VerificationConfig],
    pub scope: &'a TaskScope,
}

/// Decides the records of `prepare` once every repository is set up: the baseline revision
/// manifest, the change set with one item per writable repository, the attempt's baseline link and
/// the `attempt.workspace_prepared` event. The reply is the execution manifest the agent reads.
pub fn prepare_decision(
    facts: &PrepareFacts<'_>,
    stamp: &Stamp,
) -> Result<Decision<ExecutionManifest>, WgError> {
    let PrepareFacts {
        attempt,
        task,
        entries,
        ..
    } = *facts;
    let applicable = applicable_checks(facts.configs, facts.scope)?;
    let (verification, agent_checks) = prepare_checks(&applicable);
    let task_key = task.key.to_string();
    let plan = change_set_plan(&task_key, attempt.number, entries);

    let (baseline_id, ids) = stamp.ids().next();
    let (change_set_id, ids) = match plan.is_some() {
        true => {
            let (id, ids) = ids.next();
            (Some(id), ids)
        }
        false => (None, ids),
    };
    let item_count = plan.as_ref().map_or(0, |set| set.items.len());
    let (item_ids, ids) = take_ids(ids, item_count);
    let (event_id, _) = ids.next();

    let baseline = RevisionManifest {
        id: baseline_id,
        workspace_id: task.workspace_id,
        attempt_id: Some(attempt.id),
        kind: "baseline".to_owned(),
        entries: entries.iter().map(entry_object).collect(),
        created_at: stamp.at,
    };
    let change_set_writes = match (change_set_id, plan) {
        (Some(id), Some(items)) => {
            let set = ChangeSet {
                id,
                workspace_id: task.workspace_id,
                task_id: task.id,
                attempt_id: Some(attempt.id),
                title: items.title,
                status: ChangeSetStatus::Open,
                created_at: stamp.at,
                updated_at: stamp.at,
            };
            let rows = items
                .items
                .iter()
                .zip(item_ids)
                .map(|(item, item_id)| {
                    Id::parse_str(&item.repository_id)
                        .map(|repository_id| {
                            Write::insert(ChangeSetItem {
                                id: item_id,
                                change_set_id: id,
                                repository_id,
                                base_sha: item.base_sha.clone(),
                                head_sha: None,
                                branch: item.branch.clone(),
                                commit_shas: VectorSync::new_sync(),
                                status: ChangeSetItemStatus::Pending,
                                detail: None,
                                updated_at: stamp.at,
                            })
                        })
                        .map_err(|_| {
                            WgError::new(
                                ErrorCode::WorkgraphError,
                                format!("invalid repository id {}", item.repository_id),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            std::iter::once(Write::insert(set)).chain(rows).collect()
        }
        _ => Vec::new(),
    };
    let linked = AgentAttempt {
        baseline_manifest_id: Some(baseline_id),
        ..attempt.clone()
    };
    let bases: JsonObject = entries
        .iter()
        .filter_map(|entry| {
            entry
                .base
                .clone()
                .map(|base| (entry.repository.clone(), base))
        })
        .collect();
    let payload = AttemptWorkspacePrepared {
        repositories: entries
            .iter()
            .map(|entry| entry.repository.clone())
            .collect(),
        bases,
        isolation: facts.isolation.to_owned(),
        baseline_manifest_id: baseline_id,
        change_set_id,
    };
    let manifest = ExecutionManifest {
        task_id: task.id.to_string(),
        task_key,
        attempt_id: attempt.id.to_string(),
        workspace_dir: facts.workspace_dir.to_owned(),
        repositories: entries.to_vec(),
        expected_changes: expected_changes(facts.explicit_expected, entries),
        verification,
        agent_checks,
        baseline_manifest_id: baseline_id.to_string(),
        change_set_id: change_set_id.map(|id| id.to_string()),
    };
    let decision = std::iter::once(Write::insert(baseline))
        .chain(change_set_writes)
        .chain(std::iter::once(Write::update(linked)))
        .fold(Decision::new(()), Decision::write)
        .event(EventDraft {
            attempt_id: Some(attempt.id),
            ..EventDraft::new(
                stamp,
                event_id,
                Some(task.workspace_id),
                attempt.id,
                EventPayload::AttemptWorkspacePrepared(payload),
            )
        });
    Ok(decision.with_reply(manifest))
}

/// The first `count` ids of `ids`, and the sequence after them.
fn take_ids(ids: wg_model::stamp::IdSeq, count: usize) -> (Vec<Id>, wg_model::stamp::IdSeq) {
    (0..count).fold((Vec::new(), ids), |(found, ids), _| {
        let (id, ids) = ids.next();
        (found.into_iter().chain([id]).collect(), ids)
    })
}

/// A manifest entry as the JSON object `RevisionManifest.entries` stores.
fn entry_object(entry: &ManifestEntry) -> JsonObject {
    match entry.to_json() {
        serde_json::Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

/// What `WorktreeManager.remove` answered: whether the worktree was removed, and why not when it
/// was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub worktree_id: Id,
    pub removed: bool,
    pub reason: Option<String>,
}

/// The dict `remove` returns: `worktree_id`, `removed` and `reason` when it was not removed.
pub fn removal_out(removal: &Removal) -> Value {
    let mut fields = vec![
        ("worktree_id", id(removal.worktree_id)),
        ("removed", Value::Bool(removal.removed)),
    ];
    if let Some(reason) = &removal.reason {
        fields.push(("reason", text(reason)));
    }
    dict(fields)
}

impl Removal {
    /// A worktree that was removed.
    pub fn removed(worktree_id: Id) -> Self {
        Removal {
            worktree_id,
            removed: true,
            reason: None,
        }
    }

    /// A worktree that was kept, and why.
    pub fn refused(worktree_id: Id, reason: impl Into<String>) -> Self {
        Removal {
            worktree_id,
            removed: false,
            reason: Some(reason.into()),
        }
    }
}

/// `NotFoundError` for a worktree id.
pub fn not_found(worktree_id: Id) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("managed worktree {worktree_id} not found"),
    )
}

/// `NotFoundError` for an attempt id.
pub fn not_found_attempt(attempt_id: Id) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("attempt {attempt_id} not found"),
    )
}

/// A git failure that could not run at all, as a `GitError` of the API.
pub fn git_failure(err: GitError) -> WgError {
    WgError::new(ErrorCode::GitError, err.to_string())
}

/// A file-system failure on `path`.
pub fn io_failure(path: &Path, err: &std::io::Error) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("cannot remove {}: {err}", path.display()),
    )
}

/// `workgraph.integrations.git.worktrees.plan_repo` failures as the API raises them.
pub fn plan_failure(err: PlanError) -> WgError {
    match err {
        PlanError::ReadOnly { repository } => WgError::new(
            ErrorCode::ValidationFailed,
            format!("repository {repository} is read-only"),
        ),
        PlanError::NoCommits { repository } => WgError::new(
            ErrorCode::GitError,
            format!("repository {repository} has no commits to base an attempt on"),
        ),
    }
}
