//! Execution manifests (CORE-13): the value types, the per-repository plan of `prepare`, the
//! expected-change and change-set plans, and the outcome capture plan.
//!
//! Mirrors `ManifestEntry`, `ExecutionManifest`, `WorktreeManager.prepare` / `_prepare_repo` /
//! `_stack_request` and `capture_outcome` in `integrations/git/worktrees.py`. Every function takes
//! file contents, paths and Git facts as inputs (resolved commits, heads, changed files, commit
//! ranges); the shell performs the lease, worktree, database and file writes that the plan names.
//! The JSON produced here keeps Python's field order, so `to_pretty_json` matches pydantic's
//! `model_dump_json(indent=2)` byte for byte.

use serde_json::{Map, Value, json};
use wg_types::{ChangeSetItemStatus as I, ChangeSetStatus as C};

use crate::branch::attempt_branch_name;
use crate::change_set::applied_status;

/// How many changed files a dirty repository reports in the capture (`changed_files[:50]`).
pub const DIRTY_FILE_LIMIT: usize = 50;

/// The detail recorded on a change set item whose attempt produced no commits.
pub const NO_COMMITS_DETAIL: &str = "no commits produced";

/// One repository of an execution manifest (`ManifestEntry`), in field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub repository_id: String,
    pub repository: String,
    pub checkout_path: String,
    pub source_checkout_path: String,
    pub sha: Option<String>,
    /// Serialized as `ref`.
    pub ref_name: Option<String>,
    pub target_ref: Option<String>,
    pub access: String,
    pub isolation: String,
    pub worktree_id: Option<String>,
    pub branch: Option<String>,
    /// Why the base was chosen when a stack decided it (`stack_base` info), as a JSON object.
    pub base: Option<Value>,
}

impl ManifestEntry {
    /// The entry as a JSON object in Python's field order (`model_dump()`).
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("repository_id".into(), json!(self.repository_id));
        map.insert("repository".into(), json!(self.repository));
        map.insert("checkout_path".into(), json!(self.checkout_path));
        map.insert(
            "source_checkout_path".into(),
            json!(self.source_checkout_path),
        );
        map.insert("sha".into(), json!(self.sha));
        map.insert("ref".into(), json!(self.ref_name));
        map.insert("target_ref".into(), json!(self.target_ref));
        map.insert("access".into(), json!(self.access));
        map.insert("isolation".into(), json!(self.isolation));
        map.insert("worktree_id".into(), json!(self.worktree_id));
        map.insert("branch".into(), json!(self.branch));
        map.insert("base".into(), self.base.clone().unwrap_or(Value::Null));
        Value::Object(map)
    }

    /// Reads an entry back from its JSON object (`ManifestEntry.model_validate`). Returns `None`
    /// when a required string is missing or mistyped.
    pub fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Some(Self {
            repository_id: text("repository_id")?,
            repository: text("repository")?,
            checkout_path: text("checkout_path")?,
            source_checkout_path: text("source_checkout_path")?,
            sha: text("sha"),
            ref_name: text("ref"),
            target_ref: text("target_ref"),
            access: text("access")?,
            isolation: text("isolation")?,
            worktree_id: text("worktree_id"),
            branch: text("branch"),
            base: value.get("base").filter(|b| !b.is_null()).cloned(),
        })
    }
}

/// The execution manifest an agent reads (`ExecutionManifest`), in field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionManifest {
    pub task_id: String,
    pub task_key: String,
    pub attempt_id: String,
    pub workspace_dir: String,
    pub repositories: Vec<ManifestEntry>,
    pub expected_changes: Vec<String>,
    pub verification: Vec<Value>,
    pub agent_checks: Vec<Value>,
    pub baseline_manifest_id: String,
    pub change_set_id: Option<String>,
}

impl ExecutionManifest {
    /// The manifest as a JSON object in Python's field order.
    pub fn to_json(&self) -> Value {
        json!({
            "task_id": self.task_id,
            "task_key": self.task_key,
            "attempt_id": self.attempt_id,
            "workspace_dir": self.workspace_dir,
            "repositories": self.repositories.iter().map(ManifestEntry::to_json).collect::<Vec<_>>(),
            "expected_changes": self.expected_changes,
            "verification": self.verification,
            "agent_checks": self.agent_checks,
            "baseline_manifest_id": self.baseline_manifest_id,
            "change_set_id": self.change_set_id,
        })
    }

    /// The manifest file text (`model_dump_json(indent=2)`).
    pub fn to_pretty_json(&self) -> String {
        // Serializing a `Value` cannot fail; the fallback keeps the function total.
        serde_json::to_string_pretty(&self.to_json()).unwrap_or_default()
    }
}

/// The request for one repository, as the shell resolved it before planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRequest {
    pub target_ref: Option<String>,
    pub base: Option<Value>,
}

/// Applies the stack base to a request that did not choose its own target (`_stack_request`).
/// `stack` is the base the stack chose for this repository, if any.
pub fn with_stack_base(request: RepoRequest, stack: Option<(String, Value)>) -> RepoRequest {
    if request.target_ref.is_some() {
        return request;
    }
    match stack {
        Some((commit, info)) => RepoRequest {
            target_ref: Some(commit),
            base: Some(info),
        },
        None => request,
    }
}

/// The ref a repository's base is resolved from: the requested target, else the default branch.
/// An empty name counts as no target (`if target:`).
pub fn effective_target<'a>(
    request: &'a RepoRequest,
    default_branch: Option<&'a str>,
) -> Option<&'a str> {
    request
        .target_ref
        .as_deref()
        .or(default_branch)
        .filter(|name| !name.is_empty())
}

/// What the shell knows about one repository before planning its entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFacts<'a> {
    pub repository_id: &'a str,
    pub repository: &'a str,
    pub source_checkout_path: &'a str,
    /// The access the request asks for: `read_write`, `read_only` or another access word.
    pub access: &'a str,
    /// The repository's own access mode (`read_only` forbids writes).
    pub access_mode: &'a str,
    /// The requested isolation (`worktree`, `in_place` or `snapshot`).
    pub isolation: &'a str,
    pub request: RepoRequest,
    /// The effective target from `effective_target`, and what it resolves to (`None` when it does
    /// not resolve, or when there is no target).
    pub target: Option<&'a str>,
    pub resolved_target: Option<&'a str>,
    /// The checkout's HEAD commit and current branch.
    pub head: Option<&'a str>,
    pub current_branch: Option<&'a str>,
    /// Where the managed worktree is created (joined by the shell), and its row id once created.
    pub worktree_path: &'a str,
    pub worktree_id: Option<&'a str>,
}

/// The attempt a repository plan belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptRef<'a> {
    pub task_key: &'a str,
    pub attempt_number: i64,
    pub attempt_id: &'a str,
}

/// Why a repository could not be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// A writable attempt on a repository whose access mode is read-only.
    ReadOnly { repository: String },
    /// The base commit could not be found: no target resolved and the checkout has no HEAD.
    NoCommits { repository: String },
}

/// What the shell does for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoPlan {
    /// Write in the source checkout under an exclusive lease on the checkout.
    InPlace { entry: ManifestEntry },
    /// Create a managed worktree at `entry.checkout_path` from `entry.sha`. `entry.branch` is the
    /// branch to create (`worktree add -b`), or `None` for a detached snapshot (`--detach`).
    Worktree { entry: ManifestEntry },
}

/// Plans one repository of `prepare` (`_prepare_repo`, after the database reads).
pub fn plan_repo(facts: &RepoFacts<'_>, attempt: AttemptRef<'_>) -> Result<RepoPlan, PlanError> {
    let writable = facts.access == "read_write";
    if writable && facts.access_mode == "read_only" {
        return Err(PlanError::ReadOnly {
            repository: facts.repository.to_owned(),
        });
    }
    let base = match facts.target {
        Some(_) => facts.resolved_target.or(facts.head),
        None => facts.head,
    }
    .ok_or_else(|| PlanError::NoCommits {
        repository: facts.repository.to_owned(),
    })?;
    if facts.isolation == "in_place" && writable {
        return Ok(RepoPlan::InPlace {
            entry: ManifestEntry {
                repository_id: facts.repository_id.to_owned(),
                repository: facts.repository.to_owned(),
                checkout_path: facts.source_checkout_path.to_owned(),
                source_checkout_path: facts.source_checkout_path.to_owned(),
                sha: facts.head.map(str::to_owned),
                ref_name: facts.current_branch.map(str::to_owned),
                target_ref: facts.target.map(str::to_owned),
                access: facts.access.to_owned(),
                isolation: "in_place".to_owned(),
                worktree_id: None,
                branch: None,
                base: None,
            },
        });
    }
    let branch = writable
        .then(|| attempt_branch_name(attempt.task_key, attempt.attempt_number, attempt.attempt_id));
    let isolation = if branch.is_some() {
        "worktree"
    } else {
        "snapshot"
    };
    Ok(RepoPlan::Worktree {
        entry: ManifestEntry {
            repository_id: facts.repository_id.to_owned(),
            repository: facts.repository.to_owned(),
            checkout_path: facts.worktree_path.to_owned(),
            source_checkout_path: facts.source_checkout_path.to_owned(),
            sha: Some(base.to_owned()),
            ref_name: branch.clone(),
            target_ref: facts.target.map(str::to_owned),
            access: facts.access.to_owned(),
            isolation: isolation.to_owned(),
            worktree_id: facts.worktree_id.map(str::to_owned),
            branch,
            base: facts.request.base.clone(),
        },
    })
}

/// The directory name a repository's worktree gets under the workspace: every character outside
/// `A-Za-z0-9._-` becomes `_`.
pub fn worktree_dir_name(repository: &str) -> String {
    repository
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The expected changes: the explicit list when it is non-empty, else the writable repositories
/// in entry order (`expected_changes or [...]`).
pub fn expected_changes(explicit: &[String], entries: &[ManifestEntry]) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    entries
        .iter()
        .filter(|e| e.access == "read_write")
        .map(|e| e.repository.clone())
        .collect()
}

/// One item of the change set a manifest opens for its writable repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetItemPlan {
    pub repository_id: String,
    pub base_sha: Option<String>,
    pub branch: Option<String>,
}

/// The change set a manifest opens (`title` is `"<task key> attempt <n>"`); items start pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetPlan {
    pub title: String,
    pub items: Vec<ChangeSetItemPlan>,
}

/// The change set for the writable entries, or `None` when no entry is writable.
pub fn change_set_plan(
    task_key: &str,
    attempt_number: i64,
    entries: &[ManifestEntry],
) -> Option<ChangeSetPlan> {
    let items: Vec<ChangeSetItemPlan> = entries
        .iter()
        .filter(|e| e.access == "read_write")
        .map(|e| ChangeSetItemPlan {
            repository_id: e.repository_id.clone(),
            base_sha: e.sha.clone(),
            branch: e.branch.clone(),
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(ChangeSetPlan {
        title: format!("{task_key} attempt {attempt_number}"),
        items,
    })
}

/// What the shell observed in one repository at capture time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoObservation {
    /// HEAD of the checkout, or `None` when it is not a repository.
    pub head: Option<String>,
    /// Staged, unstaged and untracked paths, concatenated in that order. Ignored without a HEAD.
    pub changed_files: Vec<String>,
    /// `rev-list --reverse <sha>..<head>`, read only when `needs_commit_range` says so.
    pub commits: Vec<String>,
}

/// Whether the commits between the baseline and the head must be listed.
pub fn needs_commit_range(sha: Option<&str>, head: Option<&str>) -> bool {
    matches!((sha, head), (Some(sha), Some(head)) if sha != head)
}

/// A read-only repository that changed (`violations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub repository: String,
    pub commits: usize,
    pub uncommitted_files: usize,
}

/// The new state of one change set item after capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemUpdate {
    pub repository_id: String,
    pub status: I,
    pub detail: Option<String>,
}

/// Everything `capture_outcome` records from the observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePlan {
    /// The outcome manifest entries: the baseline entries with `sha` replaced by the head.
    pub outcome: Vec<ManifestEntry>,
    pub violations: Vec<Violation>,
    /// Repositories with commits that were not violations.
    pub modified_repository_ids: Vec<String>,
    /// Repository name and its changed paths (first `DIRTY_FILE_LIMIT`), for dirty repositories.
    pub dirty: Vec<(String, Vec<String>)>,
    /// Item updates for the change set's writable repositories.
    pub item_updates: Vec<ItemUpdate>,
    /// The change set status after the updates, when a change set exists.
    pub change_set_status: Option<C>,
}

/// Plans the capture of an attempt's outcome. `observations` are aligned with `entries`.
pub fn plan_capture(
    entries: &[ManifestEntry],
    observations: &[RepoObservation],
    has_change_set: bool,
) -> CapturePlan {
    let mut outcome = Vec::with_capacity(entries.len());
    let mut violations = Vec::new();
    let mut modified_repository_ids = Vec::new();
    let mut dirty = Vec::new();
    let mut item_updates = Vec::new();
    let mut statuses = Vec::new();
    for (entry, observed) in entries.iter().zip(observations) {
        let head = observed.head.as_deref();
        let changed: &[String] = if head.is_some() {
            &observed.changed_files
        } else {
            &[]
        };
        let commits: &[String] = if needs_commit_range(entry.sha.as_deref(), head) {
            &observed.commits
        } else {
            &[]
        };
        let mut recorded = entry.clone();
        recorded.sha = observed.head.clone();
        outcome.push(recorded);
        if !changed.is_empty() {
            let kept = changed.iter().take(DIRTY_FILE_LIMIT).cloned().collect();
            dirty.push((entry.repository.clone(), kept));
        }
        if entry.access == "read_only" && (!commits.is_empty() || !changed.is_empty()) {
            violations.push(Violation {
                repository: entry.repository.clone(),
                commits: commits.len(),
                uncommitted_files: changed.len(),
            });
        } else if !commits.is_empty() {
            modified_repository_ids.push(entry.repository_id.clone());
        }
        if entry.access == "read_write" {
            let (status, detail) = if commits.is_empty() {
                (I::Pending, Some(NO_COMMITS_DETAIL.to_owned()))
            } else {
                (I::Committed, None)
            };
            statuses.push(status);
            item_updates.push(ItemUpdate {
                repository_id: entry.repository_id.clone(),
                status,
                detail,
            });
        }
    }
    let change_set_status = has_change_set.then(|| applied_status(&statuses));
    CapturePlan {
        outcome,
        violations,
        modified_repository_ids,
        dirty,
        item_updates,
        change_set_status,
    }
}
