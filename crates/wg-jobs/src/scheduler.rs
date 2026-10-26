//! Scheduling decisions: the automation schedules of `automation/engine.py` (`run_scheduled`,
//! `_next_time`) and the agent scheduler's fair order and capacity (`orchestration/scheduler.py`
//! `_fair_order`, `tick`).
//!
//! Every function takes the instant it decides at; nothing reads a clock. The shell loads the
//! rule or the candidates, applies the returned step, and performs the row writes.

use std::collections::VecDeque;

use serde_json::{Map, Value};
use wg_automation::cron::Cron;
use wg_types::{Timestamp, WgError};

use crate::shift;

/// Seconds between runs of an interval schedule that names no interval (`or 3600`).
pub const DEFAULT_INTERVAL_SECONDS: i64 = 3600;

/// The project key candidates without a project share (`"-"` in the Python queue key).
const NO_PROJECT: &str = "-";

/// The next time a schedule trigger fires after `after`: the next cron minute, or the interval
/// later.
pub fn next_time(trigger: &Map<String, Value>, after: Timestamp) -> Result<Timestamp, WgError> {
    if let Some(expression) = trigger
        .get("cron")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        let next = Cron::parse(expression)?.next_after(after.to_offset_datetime())?;
        return Timestamp::from_offset_datetime(next);
    }
    let seconds = trigger
        .get("interval_seconds")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds != 0)
        .unwrap_or(DEFAULT_INTERVAL_SECONDS);
    Ok(shift(after, seconds as f64))
}

/// A schedule slot that is due: the trigger key and the `scheduled_for` input the run records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fire {
    /// `schedule:<slot>`, the idempotency key of the run.
    pub trigger_key: String,
    /// The slot in ISO form, whole seconds, as the run's `scheduled_for` input.
    pub scheduled_for: String,
}

/// What the scheduled-rule pass does for one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleStep {
    /// The rule is not a schedule: nothing changes.
    Idle,
    /// The rule has no next run yet: record this one and fire nothing.
    Initialise { next_run_at: Timestamp },
    /// The next run is still in the future.
    Wait,
    /// The slot is due: fire it, record `now` as the last scheduled instant and move the next
    /// run to `next_run_at`. The shell still skips the run when its trigger key already exists.
    Fire { fire: Fire, next_run_at: Timestamp },
}

/// Decides one enabled rule's scheduled step at `now` (`AutomationService.run_scheduled`).
pub fn schedule_step(
    trigger: &Map<String, Value>,
    next_run_at: Option<Timestamp>,
    now: Timestamp,
) -> Result<ScheduleStep, WgError> {
    if trigger.get("type").and_then(Value::as_str) != Some("schedule") {
        return Ok(ScheduleStep::Idle);
    }
    let Some(due) = next_run_at else {
        return Ok(ScheduleStep::Initialise {
            next_run_at: next_time(trigger, now)?,
        });
    };
    if due > now {
        return Ok(ScheduleStep::Wait);
    }
    let whole = due
        .to_offset_datetime()
        .replace_nanosecond(0)
        .unwrap_or(due.to_offset_datetime());
    let slot = Timestamp::from_offset_datetime(whole)?.format_isoformat();
    Ok(ScheduleStep::Fire {
        fire: Fire {
            trigger_key: format!("schedule:{slot}"),
            scheduled_for: slot,
        },
        next_run_at: next_time(trigger, now)?,
    })
}

/// A candidate for the agent scheduler: its project, its priority rank (lower runs first) and
/// the item itself. Candidates arrive already ordered by priority, then age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate<T> {
    pub project: Option<String>,
    pub rank: i64,
    pub item: T,
}

/// Interleaves candidates round-robin across projects, so one busy project cannot starve the
/// others, while the priority rank still decides each round (`_fair_order`).
pub fn fair_order<T>(candidates: Vec<Candidate<T>>) -> Vec<T> {
    fair_rounds(group_by_project(candidates))
}

/// Groups candidates into one queue per project, projects in first-appearance order.
fn group_by_project<T>(candidates: Vec<Candidate<T>>) -> Vec<VecDeque<Candidate<T>>> {
    candidates
        .into_iter()
        .fold(
            Vec::<(String, VecDeque<Candidate<T>>)>::new(),
            push_candidate,
        )
        .into_iter()
        .map(|(_, queue)| queue)
        .collect()
}

/// Appends `candidate` to the queue of its project, opening a new queue if the project is unseen.
fn push_candidate<T>(
    queues: Vec<(String, VecDeque<Candidate<T>>)>,
    candidate: Candidate<T>,
) -> Vec<(String, VecDeque<Candidate<T>>)> {
    let key = candidate
        .project
        .clone()
        .unwrap_or_else(|| NO_PROJECT.to_owned());
    let (pending, kept) = queues.into_iter().fold(
        (
            Some(candidate),
            Vec::<(String, VecDeque<Candidate<T>>)>::new(),
        ),
        |(pending, kept), (name, queue)| match pending {
            Some(item) if name == key => (
                None,
                kept.into_iter()
                    .chain([(name, queue.into_iter().chain([item]).collect())])
                    .collect(),
            ),
            pending => (pending, kept.into_iter().chain([(name, queue)]).collect()),
        },
    );
    match pending {
        Some(item) => kept
            .into_iter()
            .chain([(key, VecDeque::from([item]))])
            .collect(),
        None => kept,
    }
}

/// Emits round after round until every queue is empty (`_fair_order`).
fn fair_rounds<T>(queues: Vec<VecDeque<Candidate<T>>>) -> Vec<T> {
    if queues.iter().all(VecDeque::is_empty) {
        return Vec::new();
    }
    let (taken, kept) = fair_round(queues);
    taken.into_iter().chain(fair_rounds(kept)).collect()
}

/// One round: the head of every queue whose head has the best rank, in queue order, and the
/// queues with those heads removed.
fn fair_round<T>(queues: Vec<VecDeque<Candidate<T>>>) -> (Vec<T>, Vec<VecDeque<Candidate<T>>>) {
    let best = queues
        .iter()
        .filter_map(|queue| queue.front().map(|head| head.rank))
        .min();
    queues
        .into_iter()
        .fold((Vec::new(), Vec::new()), |(taken, kept), queue| match queue
            .front()
            .is_some_and(|head| Some(head.rank) == best)
        {
            true => {
                let (head, rest) = split_head(queue);
                (
                    taken
                        .into_iter()
                        .chain(head.map(|candidate| candidate.item))
                        .collect(),
                    kept.into_iter().chain([rest]).collect(),
                )
            }
            false => (taken, kept.into_iter().chain([queue]).collect()),
        })
}

/// A queue split into its first candidate and the rest, in order.
fn split_head<T>(queue: VecDeque<Candidate<T>>) -> (Option<Candidate<T>>, VecDeque<Candidate<T>>) {
    queue.into_iter().fold(
        (None, VecDeque::new()),
        |(head, rest), candidate| match head {
            None => (Some(candidate), rest),
            Some(_) => (head, rest.into_iter().chain([candidate]).collect()),
        },
    )
}

/// The concurrency slots still free: the configured maximum less the active attempts, never
/// negative (`tick`).
pub fn capacity(max_concurrent: i64, active: i64) -> i64 {
    (max_concurrent - active).max(0)
}
