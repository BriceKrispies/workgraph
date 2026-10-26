//! Structured decisions: a question a task is waiting on, the options on offer and the answer.
//! Mirrors `request`, `choose`, `withdraw` and `_release_task` in `core/services/decisions.py`.
//!
//! Each function decides and returns a [`DecisionPlan`]: the decision row after the change, the
//! comment it records, the events it appends (in order) and the tag change on the task. The shell
//! inserts or replaces the row, writes the comment, appends the events, applies the tag update
//! (its own `task.updated` event follows the decision events), and commits in one transaction.
//!
//! The waiting tags ([`CLEARED_TAGS`]) are dropped once none of the task's decisions is open.

use serde_json::{Map, json};
use wg_model::EventPayload;
use wg_model::entity::{DecisionOption, TaskComment, TaskDecision};
use wg_model::event::{TaskCommentRecorded, TaskDecisionChanged};
use wg_model::stamp::{Stamp, py_repr_str};
use wg_model::{Task, TaskSlice};
use wg_types::{CommentKind, ErrorCode, Permission, WgError};

use crate::comments;
use crate::promote::PARKED_TAG;
use crate::update::{TaskPatch, UpdatePlan, update_plan};

/// Tag a task carries while a decision on it is open.
pub const NEEDS_DECISION_TAG: &str = "needs-decision";
/// Tag the triage run sets alongside a structured decision.
pub const TRIAGE_NEEDS_DECISION_TAG: &str = "triage:needs-decision";
/// Dropped from a task once none of its decisions is open.
pub const CLEARED_TAGS: [&str; 3] = [NEEDS_DECISION_TAG, PARKED_TAG, TRIAGE_NEEDS_DECISION_TAG];

/// A decision carries at most this many options (`DecisionRequest.options`, `max_length=26`).
pub const MAX_OPTIONS: usize = 26;
/// Characters of the question kept in a decision event's payload.
pub const EVENT_QUESTION_CHARS: usize = 500;

/// One option as the caller gives it. `id` of `None` (or empty) is filled from the position.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptionInput {
    pub id: Option<String>,
    pub label: String,
    pub detail: String,
    pub recommended: bool,
}

/// The answer to a decision: exactly one of `option_id` and `other_text`, and an optional note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Answer<'a> {
    pub option_id: Option<&'a str>,
    pub other_text: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// An event the shell appends, in the order the plan lists it. The payload names its event type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEvent {
    pub payload: EventPayload,
}

/// What a decision change writes. The shell applies it in one transaction, in this order:
/// `comment` (when present), `events`, then `tags` (its `task.updated` event last).
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionPlan {
    /// The decision row after the change. `request` inserts it; `choose` and `withdraw` replace
    /// the stored row, whose `version` is one less.
    pub decision: TaskDecision,
    /// The comment recorded for the change, if any.
    pub comment: Option<TaskComment>,
    /// The comment's `task.comment_added` event (when there is a comment), then the decision's event.
    pub events: Vec<PlannedEvent>,
    /// The tag change on the task, when the tag set changes. The shell writes its task row and
    /// its `task.updated` event after `events`.
    pub tags: Option<UpdatePlan>,
}

/// Fills in missing option ids from position ("a", "b", ...) and refuses duplicates and more
/// than [`MAX_OPTIONS`] options (`normalize_options`).
pub fn normalize_options(options: &[OptionInput]) -> Result<Vec<DecisionOption>, WgError> {
    if options.len() > MAX_OPTIONS {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("a decision carries at most {MAX_OPTIONS} options"),
        ));
    }
    let mut out: Vec<DecisionOption> = Vec::with_capacity(options.len());
    for (position, option) in options.iter().enumerate() {
        let id = option
            .id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map_or_else(|| letter(position), str::to_owned);
        if out.iter().any(|seen| seen.id == id) {
            return Err(WgError::new(
                ErrorCode::ValidationFailed,
                format!("duplicate option id {}", py_repr_str(&id)),
            ));
        }
        out.push(DecisionOption {
            id,
            label: option.label.clone(),
            detail: option.detail.clone(),
            recommended: option.recommended,
        });
    }
    Ok(out)
}

/// "a", "b", ... "z" by position.
fn letter(position: usize) -> String {
    let offset = u8::try_from(position).unwrap_or(u8::MAX);
    char::from(b'a'.saturating_add(offset)).to_string()
}

/// Opens a decision on `task` and tags the task `needs-decision` (`request`). An empty `options`
/// list means a free-text answer.
pub fn plan_request(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    question: &str,
    options: &[OptionInput],
) -> Result<DecisionPlan, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let question = question.trim();
    if question.is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "question must not be empty",
        ));
    }
    let normalized = normalize_options(options)?;
    let (id, _) = stamp.ids().next();
    let decision = TaskDecision {
        id,
        workspace_id: task.workspace_id,
        task_id: task.id,
        question: question.to_owned(),
        options: normalized,
        status: "open".to_owned(),
        chosen_option_id: None,
        other_text: None,
        note: None,
        decided_by: None,
        decided_at: None,
        created_by: stamp.actor.to_string(),
        created_at: stamp.at,
        updated_at: stamp.at,
        version: 1,
    };
    let event = decision_event(EventPayload::TaskDecisionRequested, task, &decision);
    let tags = if has_tag(tasks, task, NEEDS_DECISION_TAG) {
        None
    } else {
        retag(stamp, tasks, task, &[NEEDS_DECISION_TAG], &[])?
    };
    Ok(DecisionPlan {
        decision,
        comment: None,
        events: vec![event],
        tags,
    })
}

/// Answers a decision with an option or free text (`choose`). An answered decision may be
/// answered again; a withdrawn one may not. `decisions` is every decision on `task`, `decision`
/// the one being answered (its stored row).
pub fn plan_choose(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    decisions: &[TaskDecision],
    decision: &TaskDecision,
    answer: Answer<'_>,
) -> Result<DecisionPlan, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    if answer.option_id.is_some() == answer.other_text.is_some() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "give exactly one of option_id and other_text",
        ));
    }
    if answer.other_text.is_some_and(|text| text.trim().is_empty()) {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "other_text must not be empty",
        ));
    }
    if decision.status == "withdrawn" {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "decision was withdrawn; request a new one",
        ));
    }
    if let Some(option_id) = answer.option_id
        && !decision.options.iter().any(|option| option.id == option_id)
    {
        let known = decision
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let known = if known.is_empty() {
            "none, answer with other_text".to_owned()
        } else {
            known
        };
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!(
                "unknown option {}; options are: {known}",
                py_repr_str(option_id)
            ),
        )
        .with_detail("option_id", json!(option_id)));
    }

    let note = answer
        .note
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_owned);
    let mut next = decision.clone();
    next.status = "decided".to_owned();
    next.chosen_option_id = answer.option_id.map(str::to_owned);
    next.other_text = answer.other_text.map(|text| text.trim().to_owned());
    next.note = note.clone();
    next.decided_by = Some(stamp.actor.to_string());
    next.decided_at = Some(stamp.at);
    next.updated_at = stamp.at;
    next.version += 1;

    let date = stamp.at.to_offset_datetime().date();
    let mut body = format!(
        "User decision {:04}-{:02}-{:02}: {} -> {}",
        date.year(),
        u8::from(date.month()),
        date.day(),
        next.question,
        answer_text(&next)
    );
    if let Some(note) = &note {
        body.push_str(&format!("\n\nNote: {note}"));
    }
    let (comment, mut events) = comment_plan(stamp, task, body);
    events.push(decision_event(
        EventPayload::TaskDecisionChosen,
        task,
        &next,
    ));
    let tags = release(stamp, tasks, task, decisions, &next)?;
    Ok(DecisionPlan {
        decision: next,
        comment: Some(comment),
        events,
        tags,
    })
}

/// Retires an open decision that no longer needs an answer (`withdraw`). A `reason` that is
/// empty is not recorded.
pub fn plan_withdraw(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    decisions: &[TaskDecision],
    decision: &TaskDecision,
    reason: Option<&str>,
) -> Result<DecisionPlan, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    if decision.status != "open" {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!(
                "only an open decision can be withdrawn; this one is {}",
                decision.status
            ),
        ));
    }
    let mut next = decision.clone();
    next.status = "withdrawn".to_owned();
    next.updated_at = stamp.at;
    next.version += 1;

    let mut body = format!("Decision withdrawn: {}", next.question);
    if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
        body.push_str(&format!("\n\nReason: {reason}"));
    }
    let (comment, mut events) = comment_plan(stamp, task, body);
    events.push(decision_event(
        EventPayload::TaskDecisionWithdrawn,
        task,
        &next,
    ));
    let tags = release(stamp, tasks, task, decisions, &next)?;
    Ok(DecisionPlan {
        decision: next,
        comment: Some(comment),
        events,
        tags,
    })
}

/// The text a decision's answer reads as in the comment: the free text, else the chosen
/// option's label, else the chosen id.
fn answer_text(decision: &TaskDecision) -> String {
    if let Some(text) = &decision.other_text {
        return text.clone();
    }
    let chosen = decision.chosen_option_id.clone().unwrap_or_default();
    decision
        .options
        .iter()
        .find(|option| option.id == chosen)
        .map_or(chosen, |option| option.label.clone())
}

/// The comment row and its `task.comment_added` event (`_comment` with kind `comment`).
fn comment_plan(stamp: &Stamp, task: &Task, body: String) -> (TaskComment, Vec<PlannedEvent>) {
    let (id, _) = stamp.ids().next();
    let comment = TaskComment {
        id,
        task_id: task.id,
        attempt_id: None,
        author: stamp.actor.to_string(),
        kind: CommentKind::Comment,
        body,
        data: Map::new(),
        created_at: stamp.at,
    };
    let event = PlannedEvent {
        payload: EventPayload::TaskCommentAdded(TaskCommentRecorded {
            comment_id: comment.id,
            kind: CommentKind::Comment,
            excerpt: comments::excerpt(&comment.body),
            extra: Map::new(),
        }),
    };
    (comment, vec![event])
}

/// The payload of a decision event. `question` is cut to [`EVENT_QUESTION_CHARS`] characters.
fn decision_event(
    payload: fn(TaskDecisionChanged) -> EventPayload,
    task: &Task,
    decision: &TaskDecision,
) -> PlannedEvent {
    let question: String = decision
        .question
        .chars()
        .take(EVENT_QUESTION_CHARS)
        .collect();
    PlannedEvent {
        payload: payload(TaskDecisionChanged {
            key: task.key.clone(),
            decision_id: decision.id,
            question,
            version: decision.version,
        }),
    }
}

/// Drops the waiting tags once no decision on the task other than `answered` is open
/// (`_release_task`). Only the tags the task carries are removed.
fn release(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    decisions: &[TaskDecision],
    answered: &TaskDecision,
) -> Result<Option<UpdatePlan>, WgError> {
    let still_open = decisions
        .iter()
        .any(|other| other.id != answered.id && other.status == "open");
    if still_open {
        return Ok(None);
    }
    let present: Vec<&str> = CLEARED_TAGS
        .iter()
        .copied()
        .filter(|tag| has_tag(tasks, task, tag))
        .collect();
    if present.is_empty() {
        return Ok(None);
    }
    retag(stamp, tasks, task, &[], &present)
}

/// Adds and removes tags through the task update plan. `None` when nothing changes.
fn retag(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    add: &[&str],
    remove: &[&str],
) -> Result<Option<UpdatePlan>, WgError> {
    let patch = TaskPatch {
        add_tags: add.iter().map(|tag| (*tag).to_owned()).collect(),
        remove_tags: remove.iter().map(|tag| (*tag).to_owned()).collect(),
        ..TaskPatch::default()
    };
    let plan = update_plan(stamp, tasks, task, &patch)?;
    Ok(plan.event.is_some().then_some(plan))
}

fn has_tag(tasks: &TaskSlice, task: &Task, tag: &str) -> bool {
    tasks.tags_of(task.id).any(|existing| existing == tag)
}
