//! The scheduler's decisions (`orchestration/scheduler.py`): which tasks may run on their own, in
//! what order, why a task may not, and how many a pass starts.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{Map, Value as Json};
use wg_exec::attempt::{ACTIVE_STATUSES, counts_as_failure};
use wg_model::TaskSlice;
use wg_model::entity::{AgentAttempt, RepositoryCheckout, Task};
use wg_model::slice::StackSlice;
use wg_registry::checkout::primary_checkout;
use wg_tasks::deps::unsatisfied_dependencies;
use wg_types::{AttemptStatus, Id, TaskPriority, TaskStatus};

/// The most candidates a pass looks at (`.limit(500)`).
pub const CANDIDATE_LIMIT: usize = 500;

/// The tag an escalated task carries.
pub const NEEDS_REVIEW: &str = "needs-review";

/// A task that may run on its own, as the scheduler lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eligible {
    pub task_id: Id,
    pub key: String,
    pub priority: TaskPriority,
    pub project_id: Option<Id>,
    pub agent: Option<String>,
    pub workspace_id: Id,
}

impl Eligible {
    /// The item as the scheduler's reports print it.
    pub fn to_json(&self) -> Json {
        Json::Object(Map::from_iter([
            ("task_id".to_owned(), Json::String(self.task_id.to_string())),
            ("key".to_owned(), Json::String(self.key.clone())),
            (
                "priority".to_owned(),
                Json::String(self.priority.as_str().to_owned()),
            ),
            (
                "project_id".to_owned(),
                self.project_id
                    .map_or(Json::Null, |id| Json::String(id.to_string())),
            ),
            (
                "agent".to_owned(),
                self.agent.clone().map_or(Json::Null, Json::String),
            ),
            (
                "workspace_id".to_owned(),
                Json::String(self.workspace_id.to_string()),
            ),
        ]))
    }
}

/// `PRIORITY_RANK`: critical first.
pub fn priority_rank(priority: TaskPriority) -> i64 {
    match priority {
        TaskPriority::Critical => 0,
        TaskPriority::High => 1,
        TaskPriority::Medium => 2,
        TaskPriority::Low => 3,
    }
}

/// What the scheduler reads about the world.
pub struct World<'a> {
    pub tasks: &'a TaskSlice,
    pub stacks: &'a StackSlice,
    /// The attempts in the statuses the scheduler counts: the active ones and the failed ones.
    pub attempts: &'a [AgentAttempt],
    /// The checkouts of each repository a candidate links.
    pub checkouts: &'a BTreeMap<Id, Vec<RepositoryCheckout>>,
}

/// `Scheduler._ineligibility`: why a task may not run, in the order the reasons are found.
pub fn ineligibility(world: &World<'_>, task: &Task, max_attempts: i64) -> Vec<String> {
    let blocked = !unsatisfied_dependencies(world.tasks, world.stacks, task.id).is_empty();
    let mine = || {
        world
            .attempts
            .iter()
            .filter(move |row| row.task_id == task.id)
    };
    let active = mine().any(|row| ACTIVE_STATUSES.contains(&row.status));
    let failed = mine()
        .filter(|row| row.parent_attempt_id.is_none() && counts_as_failure(row.status))
        .count();
    let flagged = world.tasks.tags_of(task.id).any(|tag| tag == NEEDS_REVIEW);
    let exhausted = task.status == TaskStatus::Failed
        && (i64::try_from(failed).unwrap_or(i64::MAX) >= max_attempts || flagged);
    let unavailable = world
        .tasks
        .repositories_of(task.id)
        .filter_map(|link| world.tasks.registry.repository(link.repository_id))
        .filter(|repo| {
            repo.archived_at.is_some()
                || world
                    .checkouts
                    .get(&repo.id)
                    .and_then(|rows| primary_checkout(rows, true))
                    .is_none()
        })
        .map(|repo| format!("repository {} unavailable", repo.name));
    blocked
        .then(|| "dependencies unsatisfied".to_owned())
        .into_iter()
        .chain(active.then(|| "attempt already active".to_owned()))
        .chain(exhausted.then(|| "retry budget exhausted (escalated for review)".to_owned()))
        .chain(unavailable)
        .collect()
}

/// The auto-execute candidates of `workspace` (all when `None`): ready or failed, not archived,
/// by priority then age, at most [`CANDIDATE_LIMIT`].
pub fn candidates(tasks: &TaskSlice, workspace: Option<Id>) -> Vec<&Task> {
    let mut found: Vec<&Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.auto_execute
                && task.archived_at.is_none()
                && matches!(task.status, TaskStatus::Ready | TaskStatus::Failed)
                && workspace.is_none_or(|scope| task.workspace_id == scope)
        })
        .collect();
    found.sort_by_key(|task| (priority_rank(task.priority), task.created_at));
    found.truncate(CANDIDATE_LIMIT);
    found
}

/// `Scheduler.eligible`: the candidates with no reason against them, interleaved across
/// projects, at most `limit`.
pub fn eligible(
    world: &World<'_>,
    workspace: Option<Id>,
    max_attempts: i64,
    limit: usize,
) -> Vec<Eligible> {
    let items: Vec<Eligible> = candidates(world.tasks, workspace)
        .into_iter()
        .filter(|task| ineligibility(world, task, max_attempts).is_empty())
        .map(|task| Eligible {
            task_id: task.id,
            key: task.key.to_string(),
            priority: task.priority,
            project_id: world
                .tasks
                .projects_of(task.id)
                .next()
                .map(|link| link.project_id),
            agent: task.assigned_agent.clone(),
            workspace_id: task.workspace_id,
        })
        .collect();
    fair_order(items).into_iter().take(limit).collect()
}

/// `Scheduler._fair_order`: one queue per project in first-seen order; each round takes the head
/// of every queue whose head has the best priority among the heads.
pub fn fair_order(items: Vec<Eligible>) -> Vec<Eligible> {
    let mut queues: Vec<(String, VecDeque<Eligible>)> = Vec::new();
    for item in items {
        let name = item
            .project_id
            .map_or_else(|| "-".to_owned(), |id| id.to_string());
        match queues.iter_mut().find(|(queue, _)| *queue == name) {
            Some((_, queue)) => queue.push_back(item),
            None => queues.push((name, VecDeque::from([item]))),
        }
    }
    let mut ordered = Vec::new();
    while let Some(best) = queues
        .iter()
        .filter_map(|(_, queue)| queue.front())
        .map(|head| priority_rank(head.priority))
        .min()
    {
        for (_, queue) in &mut queues {
            if queue
                .front()
                .is_some_and(|head| priority_rank(head.priority) == best)
            {
                ordered.extend(queue.pop_front());
            }
        }
    }
    ordered
}

/// `Scheduler.explain`: whether one task is eligible and the reasons it is not. The reasons about
/// the configuration and the task itself come first, the most general first.
pub fn explain(world: &World<'_>, task: &Task, execution_enabled: bool, max_attempts: i64) -> Json {
    let found = ineligibility(world, task, max_attempts);
    let reasons: Vec<String> = (!execution_enabled)
        .then(|| "agent execution is disabled".to_owned())
        .into_iter()
        .chain(
            (!matches!(task.status, TaskStatus::Ready | TaskStatus::Failed))
                .then(|| format!("status is {}", task.status)),
        )
        .chain((!task.auto_execute).then(|| "auto_execute is off".to_owned()))
        .chain(found)
        .collect();
    Json::Object(Map::from_iter([
        ("task".to_owned(), Json::String(task.key.to_string())),
        ("eligible".to_owned(), Json::Bool(reasons.is_empty())),
        (
            "reasons".to_owned(),
            Json::Array(reasons.into_iter().map(Json::String).collect()),
        ),
    ]))
}

/// How many more runs a pass may start: the configured limit less the root attempts in flight.
pub fn capacity(max_concurrent: i64, attempts: &[AgentAttempt]) -> usize {
    let active = attempts
        .iter()
        .filter(|row| ACTIVE_STATUSES.contains(&row.status) && row.parent_attempt_id.is_none())
        .count();
    usize::try_from(max_concurrent)
        .unwrap_or(0)
        .saturating_sub(active)
}

/// The attempt statuses the scheduler reads rows for.
pub const COUNTED_STATUSES: [AttemptStatus; 6] = [
    AttemptStatus::Queued,
    AttemptStatus::Preparing,
    AttemptStatus::Running,
    AttemptStatus::Verifying,
    AttemptStatus::Failed,
    AttemptStatus::TimedOut,
];

/// The report of a pass that started nothing.
pub fn idle_report(reason: &str) -> Json {
    Json::Object(Map::from_iter([
        ("started".to_owned(), Json::Array(Vec::new())),
        ("reason".to_owned(), Json::String(reason.to_owned())),
    ]))
}

/// The report of a pass: the runs it started and the errors of those it could not.
pub fn tick_report(started: Vec<(String, Id)>, errors: Vec<(String, String)>) -> Json {
    Json::Object(Map::from_iter([
        (
            "started".to_owned(),
            Json::Array(
                started
                    .into_iter()
                    .map(|(task, attempt)| {
                        Json::Object(Map::from_iter([
                            ("task".to_owned(), Json::String(task)),
                            ("attempt_id".to_owned(), Json::String(attempt.to_string())),
                        ]))
                    })
                    .collect(),
            ),
        ),
        (
            "errors".to_owned(),
            Json::Object(
                errors
                    .into_iter()
                    .map(|(k, v)| (k, Json::String(v)))
                    .collect(),
            ),
        ),
    ]))
}
