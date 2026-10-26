//! Run flow decisions (CORE-12): the state machine of one managed agent run, from the start check
//! through prepare, run, capture, attribute, verify and conclude, plus cancel, retry and rerun.
//!
//! Mirrors `orchestration/runs.py`. The shell reads the rows, runs the agent and git, and asks
//! here what to do next; every function is a pure decision over the facts it is handed. The
//! attempt-level rules (finish plans, success, exit reasons, escalation) live in
//! [`crate::attempt`] and are reused, never copied.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use wg_types::{AttemptStatus as A, ErrorCode, TaskStatus as S, WgError};

use crate::attempt::{
    ACTIVE_STATUSES, failure_exit_reason, is_success, is_terminal, task_after_attempt_end,
    unsuccessful_status,
};

/// The most progress events one attempt records (`PROGRESS_EVENT_LIMIT`).
pub const PROGRESS_EVENT_LIMIT: usize = 500;

/// The most events one recorder write keeps (`batch[-20:]`).
pub const EVENT_BATCH_LIMIT: usize = 20;

/// The longest summary an attempt keeps (`summary[:50_000]`).
pub const SUMMARY_LIMIT: usize = 50_000;

/// The longest message an attempt event keeps.
pub const EVENT_MESSAGE_LIMIT: usize = 2000;

/// The longest transition reason a failure writes (`error[:200]`).
pub const REASON_LIMIT: usize = 200;

/// The agent timeout when the config snapshot names none, in seconds.
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 3600.0;

/// The lease a managed run claims its task for, in minutes.
pub const RUN_LEASE_MINUTES: i64 = 24 * 60;

/// The stages of a managed run, in order. A run only moves forward; any stage may end in
/// [`RunStage::Concluded`] by failing or being cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunStage {
    Queued,
    Prepare,
    Run,
    Capture,
    Attribute,
    Verify,
    Concluded,
}

impl RunStage {
    /// The attempt status a run holds while in this stage.
    pub fn attempt_status(self) -> Option<A> {
        match self {
            Self::Queued => Some(A::Queued),
            Self::Prepare => Some(A::Preparing),
            Self::Run | Self::Capture | Self::Attribute => Some(A::Running),
            Self::Verify => Some(A::Verifying),
            Self::Concluded => None,
        }
    }

    /// The stage after this one on the happy path.
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Queued => Some(Self::Prepare),
            Self::Prepare => Some(Self::Run),
            Self::Run => Some(Self::Capture),
            Self::Capture => Some(Self::Attribute),
            Self::Attribute => Some(Self::Verify),
            Self::Verify => Some(Self::Concluded),
            Self::Concluded => None,
        }
    }
}

/// The gate of a stack child's verification (`stacks.gate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The task is not in a stack: the full tier runs.
    NoStack,
    /// A stack child with no fast checks: it passes without a run.
    Pass,
    /// A stack child: only the fast tier runs, the full tier runs on the integration tip.
    Fast,
}

impl Gate {
    /// Reads the gate string the stack service returns; `None` for an unknown string.
    pub fn parse(gate: Option<&str>) -> Option<Self> {
        match gate {
            None => Some(Self::NoStack),
            Some("pass") => Some(Self::Pass),
            Some("fast") => Some(Self::Fast),
            Some(_) => None,
        }
    }
}

/// Checks that agent execution is enabled (`_require_execution`).
pub fn require_execution(enabled: bool) -> Result<(), WgError> {
    if enabled {
        Ok(())
    } else {
        Err(WgError::new(
            ErrorCode::PermissionDenied,
            "agent code execution is disabled; opt in with `workgraph config set execution.enabled true`",
        ))
    }
}

/// What a managed `run start` does with the task before queueing the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedStart {
    /// The task is ready or failed and must be claimed for the agent first.
    pub claim_first: bool,
}

/// Checks a managed run start. A ready or failed task is claimed, a claimed task is used as it is;
/// an active attempt blocks the start unless this run is a child of another attempt.
pub fn managed_start_plan(
    task_key: &str,
    task: S,
    active_attempt_number: Option<i64>,
    has_parent_attempt: bool,
) -> Result<ManagedStart, WgError> {
    let claim_first = match task {
        S::Ready | S::Failed => true,
        S::Claimed => false,
        _ => {
            return Err(invalid(format!(
                "task {task_key} is {task}; agent runs need a ready, failed or claimed task"
            )));
        }
    };
    if let (Some(number), false) = (active_attempt_number, has_parent_attempt) {
        return Err(invalid(format!(
            "task {task_key} already has an active attempt #{number}"
        )));
    }
    Ok(ManagedStart { claim_first })
}

/// Whether an attempt in `status` blocks a new attempt of its task.
pub fn blocks_new_attempt(status: A) -> bool {
    ACTIVE_STATUSES.contains(&status)
}

/// The resource keys the run's job locks: the run slot, the agent, and in place the task's repos.
pub fn resource_keys(agent: &str, isolation: &str, task_id: &str) -> Vec<String> {
    let mut keys = vec!["run".to_owned(), format!("agent:{agent}")];
    if isolation == "in_place" {
        keys.push(format!("exclusive:task-repos:{task_id}"));
    }
    keys
}

/// Checks the isolation an external start asks for.
pub fn check_isolation(isolation: Option<&str>) -> Result<(), WgError> {
    match isolation {
        None | Some("worktree" | "in_place") => Ok(()),
        Some(other) => Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("isolation must be worktree or in_place, not {other:?}"),
        )),
    }
}

/// Checks that an attempt can be retried: it must be finished and not a success.
pub fn retry_check(status: A) -> Result<(), WgError> {
    if !is_terminal(status) {
        return Err(invalid("only finished attempts can be retried"));
    }
    if status == A::Succeeded {
        return Err(invalid("the attempt succeeded; use rerun to reproduce it"));
    }
    Ok(())
}

/// One repository of an attempt's baseline manifest, with what the shell found out about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineEntry {
    pub repository_id: String,
    pub repository: String,
    pub access: String,
    pub sha: Option<String>,
    /// Whether the source checkout is a repository that still holds `sha`.
    pub revision_available: bool,
}

/// A repository request of a rerun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRequest {
    pub repository_id: String,
    pub access: String,
    pub target_ref: Option<String>,
}

/// A revision a rerun could not reproduce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingRevision {
    pub repository: String,
    pub sha: String,
}

/// What a rerun reuses from the baseline (`rerun`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunPlan {
    pub repositories: Vec<RepoRequest>,
    pub missing: Vec<MissingRevision>,
}

impl RerunPlan {
    /// Whether every baseline revision was reproduced.
    pub fn reproduced_baseline(&self) -> bool {
        self.missing.is_empty()
    }
}

/// The note every rerun carries.
pub const RERUN_NOTE: &str =
    "Agent/model execution is nondeterministic: inputs are reproduced, outputs may differ.";

/// Plans a rerun: entries whose revision is still available are pinned to it, the rest start from
/// the current head and are reported missing.
pub fn rerun_plan(entries: &[BaselineEntry]) -> RerunPlan {
    let mut plan = RerunPlan {
        repositories: Vec::new(),
        missing: Vec::new(),
    };
    for entry in entries {
        let pinned = entry.sha.as_ref().filter(|_| entry.revision_available);
        if pinned.is_none() {
            plan.missing.push(MissingRevision {
                repository: if entry.repository.is_empty() {
                    "?".to_owned()
                } else {
                    entry.repository.clone()
                },
                sha: entry.sha.clone().unwrap_or_else(|| "None".to_owned()),
            });
        }
        plan.repositories.push(RepoRequest {
            repository_id: entry.repository_id.clone(),
            access: entry.access.clone(),
            target_ref: pinned.cloned(),
        });
    }
    plan
}

/// What `cancel` does to an attempt (`cancel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelPlan {
    /// The attempt already finished: return it unchanged, record nothing.
    Unchanged,
    /// A queued attempt is finished as cancelled on the spot.
    CancelQueued {
        /// The queued job is cancelled too (only if the job is still queued).
        cancel_job: bool,
    },
    /// A started attempt is asked to stop; its worker concludes it.
    RequestStop {
        /// There is a job to flag for cancellation.
        flag_job: bool,
    },
}

/// Decides a cancel request. `job_status` is the status of the attempt's `agent_run` job, if any.
pub fn cancel_plan(attempt: A, job_status: Option<&str>) -> CancelPlan {
    if is_terminal(attempt) {
        return CancelPlan::Unchanged;
    }
    if attempt == A::Queued {
        return CancelPlan::CancelQueued {
            cancel_job: job_status == Some("queued"),
        };
    }
    CancelPlan::RequestStop {
        flag_job: job_status.is_some(),
    }
}

/// Whether a cancelled queued attempt returns its task to ready.
pub fn cancel_releases_task(task: S) -> bool {
    matches!(task, S::Claimed | S::Running)
}

/// What the job handler does on entry (`execute`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteEntry {
    /// The attempt already finished: do nothing and report this skip.
    Skip(String),
    /// Cancellation was requested before the run began.
    CancelBeforeStart,
    /// Go on to the provider check and prepare.
    Proceed,
}

/// Decides how the handler starts, given the attempt's status and cancel flag.
pub fn execute_entry(attempt: A, cancel_requested: bool) -> ExecuteEntry {
    if is_terminal(attempt) {
        ExecuteEntry::Skip(format!("attempt already {attempt}"))
    } else if cancel_requested {
        ExecuteEntry::CancelBeforeStart
    } else {
        ExecuteEntry::Proceed
    }
}

/// A run that ends before or while the agent runs, without a conclusion step (`_finalize_failure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyEnd {
    pub status: A,
    pub exit_reason: String,
    pub error: String,
}

impl EarlyEnd {
    /// The cancelled-before-start ending.
    pub fn cancelled_before_start() -> Self {
        Self {
            status: A::Cancelled,
            exit_reason: "cancelled".to_owned(),
            error: "cancelled before start".to_owned(),
        }
    }

    /// The provider could not be used.
    pub fn provider_unavailable(message: &str) -> Self {
        Self {
            status: A::Failed,
            exit_reason: "provider_unavailable".to_owned(),
            error: message.to_owned(),
        }
    }

    /// Worktree preparation failed.
    pub fn preparation_failed(message: &str) -> Self {
        Self {
            status: A::Failed,
            exit_reason: "workspace_preparation_failed".to_owned(),
            error: format!("workspace preparation failed: {message}"),
        }
    }

    /// An external attempt's start failed.
    pub fn external_start_failed(message: &str) -> Self {
        Self {
            status: A::Failed,
            exit_reason: "start_failed".to_owned(),
            error: format!("external attempt start failed: {message}"),
        }
    }

    /// The task status this ending leaves behind, if the task is held (`_finalize_failure`).
    pub fn task_after(&self, task: S) -> Option<S> {
        task_after_attempt_end(task, self.status)
    }

    /// The reason text written on the task transition.
    pub fn reason(&self) -> String {
        truncate_chars(&self.error, REASON_LIMIT)
    }
}

/// The agent result a crashed or unavailable provider is turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticResult {
    pub status: &'static str,
    pub error: String,
    pub exit_reason: &'static str,
}

/// The result recorded when the provider is unavailable at run time.
pub fn provider_unavailable_result(message: &str) -> SyntheticResult {
    SyntheticResult {
        status: "failed",
        error: message.to_owned(),
        exit_reason: "provider_unavailable",
    }
}

/// The result recorded when the adapter crashes: the crash is preserved as a failed attempt.
pub fn adapter_crash_result(kind: &str, message: &str) -> SyntheticResult {
    SyntheticResult {
        status: "failed",
        error: format!("adapter crashed: {kind}: {message}"),
        exit_reason: "adapter_crash",
    }
}

/// One repository checkout of a prepared manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub path: PathBuf,
    pub read_write: bool,
}

/// Where the agent runs and which other checkouts it may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workdir {
    pub workdir: PathBuf,
    pub additional_dirs: Vec<PathBuf>,
}

/// Picks the agent's working directory: the first writable checkout, else the workspace
/// directory. Every other checkout is an additional directory.
pub fn choose_workdir(checkouts: &[Checkout], workspace_dir: &Path) -> Workdir {
    let workdir = checkouts
        .iter()
        .find(|c| c.read_write)
        .map_or_else(|| workspace_dir.to_path_buf(), |c| c.path.clone());
    let additional_dirs = checkouts
        .iter()
        .map(|c| c.path.clone())
        .filter(|p| *p != workdir)
        .collect();
    Workdir {
        workdir,
        additional_dirs,
    }
}

/// The environment variables handed to the agent, empty values dropped. `home` is the portable
/// home, set only when `WORKGRAPH_HOME` is.
pub fn agent_env(
    attempt_id: &str,
    task_id: &str,
    task_key: &str,
    home: Option<&str>,
    manifest_path: &str,
) -> Vec<(String, String)> {
    [
        ("WORKGRAPH_ATTEMPT_ID", attempt_id),
        ("WORKGRAPH_TASK_ID", task_id),
        ("WORKGRAPH_TASK_KEY", task_key),
        ("WORKGRAPH_HOME", home.unwrap_or("")),
        ("WORKGRAPH_MANIFEST", manifest_path),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// The agent timeout from the config snapshot: zero or absent is the default.
pub fn timeout_seconds(configured: Option<f64>) -> f64 {
    configured
        .filter(|t| *t != 0.0)
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
}

/// The usage and cost fields recorded from the provider's report. Sources are tagged with the
/// adapter that reported them.
pub fn usage_fields(
    usage: &Map<String, Value>,
    cost_usd: Option<f64>,
    adapter: &str,
) -> Map<String, Value> {
    let mut fields = Map::new();
    if !usage.is_empty() {
        fields.insert("usage".into(), Value::Object(usage.clone()));
        fields.insert("usage_source".into(), json!(format!("{adapter}_reported")));
    }
    if let Some(cost) = cost_usd {
        fields.insert("cost_usd".into(), json!(cost));
        fields.insert("cost_source".into(), json!(format!("{adapter}_reported")));
    }
    fields
}

/// What capturing and attributing the attempt's changes found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureFacts {
    /// Read-only repositories that were modified.
    pub violations: Option<String>,
    /// The error that stopped the capture, if it failed.
    pub capture_error: Option<String>,
}

impl CaptureFacts {
    fn has_violations(&self) -> bool {
        self.violations.as_deref().is_some_and(|v| !v.is_empty())
    }
}

/// Whether the attempt's commits should be linked: only when the outcome was captured.
pub fn should_link_commits(facts: &CaptureFacts) -> bool {
    facts.capture_error.is_none()
}

/// How an attempt concludes before verification (`conclude_attempt`, first transaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conclusion {
    /// The agent's success stands: record usage, summary, exit code and duration, then go on to
    /// complete the task.
    Succeeded,
    /// The attempt finishes now.
    Failed {
        attempt: A,
        exit_reason: String,
        /// The error text, `agent failed` when empty.
        error: String,
    },
}

/// Decides the conclusion from the agent's result status and what capture found. An unknown
/// status is a validation error.
pub fn conclude(
    status: &str,
    error: Option<&str>,
    facts: &CaptureFacts,
) -> Result<Conclusion, WgError> {
    let attempt = unsuccessful_status(status).ok_or_else(|| {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!("unknown agent result status {status:?}"),
        )
    })?;
    if is_success(
        status,
        facts.has_violations(),
        facts.capture_error.is_some(),
    ) {
        return Ok(Conclusion::Succeeded);
    }
    let mut text = error.unwrap_or("").to_owned();
    if facts.has_violations() {
        let violations = facts.violations.as_deref().unwrap_or("");
        text = format!("read-only repositories were modified: {violations}. {text}")
            .trim()
            .to_owned();
    }
    if let Some(capture) = &facts.capture_error {
        text = format!("could not record the attempt's changes: {capture}. {text}")
            .trim()
            .to_owned();
    }
    if text.is_empty() {
        text = "agent failed".to_owned();
    }
    Ok(Conclusion::Failed {
        attempt,
        exit_reason: failure_exit_reason(status, facts.capture_error.is_some()),
        error: text,
    })
}

/// The reason written on a task transition after an attempt ends.
pub fn task_reason(number: i64, attempt: A) -> String {
    format!("attempt #{number} {attempt}")
}

/// The summary an attempt keeps: capped, and the old summary when the new one is empty.
pub fn kept_summary(new: Option<&str>, old: Option<&str>) -> Option<String> {
    match new {
        Some(s) if !s.is_empty() => Some(truncate_chars(s, SUMMARY_LIMIT)),
        _ => old.map(str::to_owned),
    }
}

/// What happens after the agent's success stands (`conclude_attempt`, second transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostSuccess {
    /// The task needs verification: the gate decides the next step.
    Verify,
    /// The task did not wait for verification: the attempt succeeds at once and the worktrees
    /// are released.
    SucceedNow,
}

/// Whether the task is completed from the agent's summary: only a running or claimed task is.
pub fn completes_task(task: S) -> bool {
    matches!(task, S::Running | S::Claimed)
}

/// Decides what follows a stood success, from the task status after completing it.
pub fn post_success(task_after_complete: S) -> PostSuccess {
    if task_after_complete == S::AwaitingVerification {
        PostSuccess::Verify
    } else {
        PostSuccess::SucceedNow
    }
}

/// The verification step a gate calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStep {
    /// No fast checks: finish the attempt as succeeded and mark the stack item ready, no run.
    SkipMarkReady,
    /// Run verification, optionally restricted to the fast tier.
    Run { fast_only: bool },
}

/// Chooses the verification step for a gate.
pub fn verify_step(gate: Gate) -> VerifyStep {
    match gate {
        Gate::Pass => VerifyStep::SkipMarkReady,
        Gate::Fast => VerifyStep::Run { fast_only: true },
        Gate::NoStack => VerifyStep::Run { fast_only: false },
    }
}

/// Whether a finished verification run marks its stack item ready.
pub fn marks_ready(gate: Gate, run_status: &str) -> bool {
    gate == Gate::Fast && run_status == "passed"
}

/// What a verification error does to the attempt: it fails with `verification_error` unless it
/// has already finished.
pub fn verification_error_end(attempt: A, message: &str) -> Option<EarlyEnd> {
    (!is_terminal(attempt)).then(|| EarlyEnd {
        status: A::Failed,
        exit_reason: "verification_error".to_owned(),
        error: message.to_owned(),
    })
}

/// Whether an external submit additionally needs the stack-write permission: the task is in a stack.
pub fn submit_needs_stack_write(gate: Option<&str>) -> bool {
    gate.is_some()
}

/// Whether a streamed event is recorded (`_EventRecorder.__call__`): log events stop once the
/// attempt has recorded more than the limit.
pub fn records_event(kind: &str, recorded: usize) -> bool {
    !(kind == "log" && recorded > PROGRESS_EVENT_LIMIT)
}

/// Whether the recorder writes its pending events now: progress, result and session events always
/// do, others once a second has passed since the last write.
pub fn flush_due(kind: &str, since_last_write_seconds: f64) -> bool {
    matches!(kind, "progress" | "result" | "session") || since_last_write_seconds > 1.0
}

/// How many of `pending` events a write records, given `recorded` so far. Zero when the limit is
/// reached, in which case the pending events are dropped.
pub fn events_to_write(pending: usize, recorded: usize) -> usize {
    if pending == 0 || recorded >= PROGRESS_EVENT_LIMIT {
        0
    } else {
        pending.min(EVENT_BATCH_LIMIT)
    }
}

/// The event type of a streamed agent event.
pub fn event_type(kind: &str) -> String {
    format!("attempt.{kind}")
}

/// The session provider key an adapter's sessions are bound under.
pub fn session_provider_key<'a>(adapter: &str, provider: &'a str) -> &'a str {
    if adapter == "claude_code" {
        "claude_code"
    } else {
        provider
    }
}

/// Whether the cancel checker consults the database now: at most every two seconds.
pub fn cancel_poll_due(since_last_poll_seconds: f64) -> bool {
    since_last_poll_seconds > 2.0
}

/// Truncates to at most `limit` characters, as Python slicing does.
pub fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn invalid(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::InvalidTransition, message)
}
