//! The decision commands as pure functions (`core/services/decisions.py` and the request models
//! `DecisionRequest`, `DecisionOptionIn`, `DecisionChoose`, `DecisionWithdraw`): request, choose
//! and withdraw as decisions over the task world, the output values of every read, and the
//! readers of the request bodies the interfaces hand over.
//!
//! The pure plans are [`crate::core`]; this module resolves the references, applies the plan as
//! writes and events in the order the Python service writes them (the decision row, the comment,
//! the decision events, then the task's tag update with its `task.updated` event) and assembles
//! the `DecisionOut` and `DecisionWithTaskOut` values.
//!
//! The submodules are the feature's Pure spec: [`dto`] (the DTOs), [`sql`] (the statement text and
//! row decoding) and [`view`] (the human views and the prefix lookup).

pub mod args;
pub mod dto;
pub mod shapes;
pub mod sql;
pub mod view;

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::entity::{Task, TaskComment, TaskDecision};
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, EventDraft, EventPayload, Write};
use wg_registry::resolve;
use wg_runtimespec::tasks::requests::{component, run};
use wg_runtimespec::tasks::writes::tag_writes;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::{ErrorCode, Id, Permission, WgError, parse_id};
use wg_validate::Model;
use wg_wire::convention::dict;

use self::dto::{decision, decision_task, decision_with_task};
pub use self::sql::DecisionFilter;
use self::sql::DecisionWithTask;
use crate::core::{
    Answer, DecisionPlan, OptionInput, check_answer, check_request, plan_choose, plan_request,
    plan_withdraw,
};

/// What a decision on one existing decision reads: the task world of its workspace, the stored
/// decision row and every decision on its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionWorld {
    pub world: TaskWorld,
    pub decision: TaskDecision,
    pub siblings: Vec<TaskDecision>,
}

/// The `DecisionFilter` a status name selects; `None` for a name that is not one.
#[must_use]
pub fn filter_of(status: &str) -> Option<DecisionFilter> {
    match status {
        "open" => Some(DecisionFilter::Open),
        "decided" => Some(DecisionFilter::Decided),
        "withdrawn" => Some(DecisionFilter::Withdrawn),
        "all" => Some(DecisionFilter::All),
        _ => None,
    }
}

/// The decision id a reference names: only the hyphenated lowercase form is a primary key.
#[must_use]
pub fn decision_key(reference: &str) -> Option<Id> {
    parse_id(reference)
        .ok()
        .filter(|id| id.hyphenated().to_string() == reference)
}

/// `DecisionService._load`: the stored decision, or `not_found` when there is none or it belongs
/// to another workspace than `scope`.
pub fn locate(
    found: Option<TaskDecision>,
    scope: Option<Id>,
    reference: &str,
) -> Result<TaskDecision, WgError> {
    found
        .filter(|row| scope.is_none_or(|ws| row.workspace_id == ws))
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("decision {} not found", wg_model::py_repr_str(reference)),
            )
            .with_detail("ref", Json::String(reference.to_owned()))
        })
}

// ---------------------------------------------------------------- request bodies

fn option_model() -> Result<Model, WgError> {
    component(
        "DecisionOptionIn",
        &["id", "label", "detail", "recommended"],
    )
}

/// Runs one option through the `DecisionOptionIn` model, as `_parse_option` builds it: whitespace
/// stripped, lengths checked. The value is the validated `model_dump`.
pub fn validate_option(option: &Json) -> Result<Json, WgError> {
    run(&option_model()?, option).map(Json::Object)
}

/// The options of a request body or tool arguments, as the plan takes them. A missing `detail` is
/// empty and a missing `recommended` is false.
#[must_use]
pub fn read_options(options: &[Json]) -> Vec<OptionInput> {
    options
        .iter()
        .map(|option| OptionInput {
            id: option.get("id").and_then(Json::as_str).map(str::to_owned),
            label: option
                .get("label")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            detail: option
                .get("detail")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            recommended: option
                .get("recommended")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

/// The question and options of a validated `DecisionRequest` body (or the arguments of
/// `task_request_decision`). Options that are absent or null are none.
#[must_use]
pub fn request_parts(body: &Json) -> (String, Vec<Json>) {
    (
        body.get("question")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        body.get("options")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default(),
    )
}

/// The three members of a validated `DecisionChoose` body: `option_id`, `other_text`, `note`.
#[must_use]
pub fn choose_parts(body: &Json) -> [Option<String>; 3] {
    ["option_id", "other_text", "note"]
        .map(|name| body.get(name).and_then(Json::as_str).map(str::to_owned))
}

/// The reason of a validated `DecisionWithdraw` body; the body may be absent.
#[must_use]
pub fn withdraw_reason(body: &Json) -> Option<String> {
    body.get("reason").and_then(Json::as_str).map(str::to_owned)
}

/// The status filter and limit of `GET /decisions`, from the validated query parameters.
#[must_use]
pub fn list_args(params: &Map<String, Json>) -> (DecisionFilter, i64) {
    (
        params
            .get("status")
            .and_then(Json::as_str)
            .and_then(filter_of)
            .unwrap_or(DecisionFilter::Open),
        params.get("limit").and_then(Json::as_i64).unwrap_or(200),
    )
}

// ---------------------------------------------------------------- outputs

/// `DecisionOut`.
#[must_use]
pub fn decision_out(row: &TaskDecision, task_key: &str) -> Value {
    decision::from_model(row, task_key)
}

/// `DecisionWithTaskOut`.
#[must_use]
pub fn with_task_out(row: &DecisionWithTask) -> Value {
    let task = &row.task;
    let brief = decision_task::from_brief(
        &task.key.to_string(),
        &task.title,
        task.status.as_str(),
        task.priority.as_str(),
        &task.description,
        &task.tags,
    );
    decision_with_task::from_model(&row.decision, &task.key.to_string(), brief)
}

/// The `DecisionWithTaskOut` list of `DecisionService.list`.
#[must_use]
pub fn list_out(rows: &[DecisionWithTask]) -> Value {
    Value::List(rows.iter().map(with_task_out).collect())
}

/// The `DecisionOut` list of `DecisionService.for_task`.
#[must_use]
pub fn for_task_out(task_key: &str, rows: &[TaskDecision]) -> Value {
    Value::List(rows.iter().map(|row| decision_out(row, task_key)).collect())
}

/// `{"items": [...]}`, the body of the list routes.
#[must_use]
pub fn items(list: Value) -> Value {
    dict(vec![("items", list)])
}

/// `{"decisions": [...]}`, the result of the list tools.
#[must_use]
pub fn decisions(list: Value) -> Value {
    dict(vec![("decisions", list)])
}

// ---------------------------------------------------------------- writes

/// The ids a plan's events take: after the one id the plan drew first, in order.
fn event_ids(stamp: &Stamp, count: usize) -> (Vec<Id>, IdSeq) {
    let (_, after_plan) = stamp.ids().next();
    (0..count).fold((Vec::new(), after_plan), |(ids, seq), _| {
        let (id, next) = seq.next();
        (ids.into_iter().chain([id]).collect(), next)
    })
}

fn comment_write(comment: Option<TaskComment>) -> Vec<Write> {
    comment.map(Write::insert).into_iter().collect()
}

/// Applies a plan: the decision row (`insert` for a new one), the comment, the decision events
/// and the task's tag update, in the order the Python service writes them. The reply is the
/// decision as `DecisionOut`.
fn apply(
    world: &TaskWorld,
    stamp: &Stamp,
    task: &Task,
    plan: DecisionPlan,
    insert: bool,
) -> Decision<Value> {
    let tasks = &world.tasks;
    let row_write = if insert {
        Write::insert(plan.decision.clone())
    } else {
        Write::update(plan.decision.clone())
    };
    let (ids, after_events) = event_ids(stamp, plan.events.len());
    let events: Vec<EventDraft> = plan
        .events
        .iter()
        .zip(ids)
        .map(|(planned, id)| {
            EventDraft::new(
                stamp,
                id,
                Some(task.workspace_id),
                task.id,
                planned.payload.clone(),
            )
        })
        .collect();
    let base = std::iter::once(row_write)
        .chain(comment_write(plan.comment))
        .fold(Decision::new(()), Decision::write);
    let tagged = match &plan.tags {
        None => base,
        Some(update) => {
            let before: Vec<String> = tasks.tags_of(task.id).map(str::to_owned).collect();
            let rows = if update.task != *task {
                vec![Write::update(update.task.clone())]
            } else {
                Vec::new()
            };
            let tags = update
                .tags
                .as_deref()
                .map(|after| tag_writes(task, &before, after))
                .unwrap_or_default();
            rows.into_iter().chain(tags).fold(base, Decision::write)
        }
    };
    let with_events = events.into_iter().fold(tagged, Decision::event);
    let reply = decision_out(&plan.decision, &task.key.to_string());
    let (update_id, _) = after_events.next();
    let finished = match plan.tags.as_ref().and_then(|update| update.event.clone()) {
        Some(payload) => with_events.event(EventDraft::new(
            stamp,
            update_id,
            Some(task.workspace_id),
            task.id,
            EventPayload::TaskUpdated(payload),
        )),
        None => with_events,
    };
    finished.with_reply(reply)
}

/// `DecisionService.request`: a decision on the task `reference` names.
pub fn request(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Option<Id>,
    reference: &str,
    question: &str,
    options: &[Json],
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    check_request(question, &[])?;
    let validated = options
        .iter()
        .map(validate_option)
        .collect::<Result<Vec<_>, _>>()?;
    let inputs = read_options(&validated);
    check_request(question, &inputs)?;
    let task = resolve::task(&world.tasks, scope, reference)?;
    let plan = plan_request(stamp, &world.tasks, task, question, &inputs)?;
    Ok(apply(world, stamp, task, plan, true))
}

/// The checks `choose` makes before it loads the decision.
pub fn check_choose(
    stamp: &Stamp,
    option_id: Option<&str>,
    other_text: Option<&str>,
) -> Result<(), WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    check_answer(Answer {
        option_id,
        other_text,
        note: None,
    })
}

/// The task a loaded decision belongs to.
fn task_of(load: &DecisionWorld) -> Result<&Task, WgError> {
    load.world
        .tasks
        .task(load.decision.task_id)
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "the decision's task vanished"))
}

/// `DecisionService.choose`: answers the loaded decision.
pub fn choose(
    load: &DecisionWorld,
    stamp: &Stamp,
    answer: Answer<'_>,
) -> Result<Decision<Value>, WgError> {
    let task = task_of(load)?;
    let plan = plan_choose(
        stamp,
        &load.world.tasks,
        task,
        &load.siblings,
        &load.decision,
        answer,
    )?;
    Ok(apply(&load.world, stamp, task, plan, false))
}

/// `DecisionService.withdraw`: retires the loaded decision.
pub fn withdraw(
    load: &DecisionWorld,
    stamp: &Stamp,
    reason: Option<&str>,
) -> Result<Decision<Value>, WgError> {
    let task = task_of(load)?;
    let plan = plan_withdraw(
        stamp,
        &load.world.tasks,
        task,
        &load.siblings,
        &load.decision,
        reason,
    )?;
    Ok(apply(&load.world, stamp, task, plan, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_hyphenated_lowercase_form_names_a_decision() {
        let id = "0162373c-c9fb-48aa-9672-78718a1604d3";
        assert!(decision_key(id).is_some());
        assert!(decision_key(&id.to_uppercase()).is_none());
        assert!(decision_key(&id.replace('-', "")).is_none());
        assert!(decision_key("deadbeef").is_none());
    }

    #[test]
    fn a_decision_that_is_not_found_names_its_reference() {
        let error = locate(None, None, "deadbeef").err();
        assert_eq!(
            error.map(|e| e.message),
            Some("decision 'deadbeef' not found".to_owned())
        );
    }

    #[test]
    fn options_default_their_detail_and_flag() {
        let read = read_options(&[
            json!({"label": "A"}),
            json!({"id": "x", "label": "B", "recommended": true}),
        ]);
        assert_eq!(read[0].detail, "");
        assert!(!read[0].recommended);
        assert_eq!(read[1].id.as_deref(), Some("x"));
        assert!(read[1].recommended);
    }

    #[test]
    fn the_list_defaults_to_open_decisions() {
        let (filter, limit) = list_args(&Map::new());
        assert_eq!((filter, limit), (DecisionFilter::Open, 200));
    }
}
