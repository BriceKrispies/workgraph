//! Structured decisions (`core/services/decisions.py`): the service cases of
//! `tests/integration/test_decisions.py` as plans over slices. A small in-test shell applies each
//! plan the way the store does: the decision row, the comment, the events, then the tag update.

mod common;

use common::{id, stamp, task, tasks_slice};
use rpds::RedBlackTreeSetSync;
use wg_model::entity::{TaskComment, TaskDecision, TaskTag};
use wg_model::stamp::Stamp;
use wg_model::{Task, TaskSlice};
use wg_tasks::decisions::{
    Answer, DecisionPlan, OptionInput, PlannedEvent, plan_choose, plan_request, plan_withdraw,
};
use wg_types::{Actor, ErrorCode, Permission, TaskStatus, WgError};

const ACTOR: &str = "human:tester";

fn opt(label: &str, recommended: bool) -> OptionInput {
    OptionInput {
        id: None,
        label: label.to_owned(),
        detail: String::new(),
        recommended,
    }
}

fn options() -> Vec<OptionInput> {
    vec![opt("Use SQLite", true), opt("Use Postgres", false)]
}

fn code_of<T: std::fmt::Debug>(result: Result<T, WgError>) -> ErrorCode {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(error) => error.code,
    }
}

/// The store's view of one task: its row, its tags, its decisions and its comments and events.
struct World {
    task: Task,
    tags: Vec<String>,
    decisions: Vec<TaskDecision>,
    comments: Vec<TaskComment>,
    events: Vec<PlannedEvent>,
    /// Task updates applied (each one a `task.updated` event on the shell side).
    task_updates: usize,
    /// Seconds after the epoch of the next decision.
    clock: i64,
    /// Changes the entropy of each decision so the ids it derives differ.
    seed: u8,
}

impl World {
    fn new(tags: &[&str]) -> Self {
        World {
            task: task("WG", 1, TaskStatus::Ready, None),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            decisions: Vec::new(),
            comments: Vec::new(),
            events: Vec::new(),
            task_updates: 0,
            clock: 100,
            seed: 0,
        }
    }

    fn slice(&self) -> TaskSlice {
        let rows = self
            .tags
            .iter()
            .map(|tag| TaskTag {
                task_id: self.task.id,
                tag: tag.clone(),
            })
            .collect();
        tasks_slice(vec![self.task.clone()], rows, vec![])
    }

    fn stamp(&mut self) -> Stamp {
        self.clock += 60;
        self.seed += 1;
        Stamp {
            at: stamp(self.clock),
            entropy: [self.seed; 32],
            actor: Actor::parse(ACTOR).unwrap_or_else(|e| panic!("{e}")),
            permissions: RedBlackTreeSetSync::new_sync().insert(Permission::TaskWrite),
            correlation: id(9),
            causation: None,
            attempt: None,
            automation_depth: 0,
        }
    }

    fn request(&mut self, question: &str, options: &[OptionInput]) -> TaskDecision {
        let stamp = self.stamp();
        let plan = plan_request(&stamp, &self.slice(), &self.task, question, options)
            .unwrap_or_else(|e| panic!("{e}"));
        self.apply(plan)
    }

    fn choose(&mut self, decision_id: &TaskDecision, answer: Answer<'_>) -> TaskDecision {
        let stamp = self.stamp();
        let slice = self.slice();
        let decisions = self.decisions.clone();
        let plan = plan_choose(&stamp, &slice, &self.task, &decisions, decision_id, answer)
            .unwrap_or_else(|e| panic!("{e}"));
        self.apply(plan)
    }

    fn try_choose(
        &mut self,
        decision: &TaskDecision,
        answer: Answer<'_>,
    ) -> Result<DecisionPlan, WgError> {
        let stamp = self.stamp();
        plan_choose(
            &stamp,
            &self.slice(),
            &self.task,
            &self.decisions.clone(),
            decision,
            answer,
        )
    }

    fn withdraw(&mut self, decision: &TaskDecision, reason: Option<&str>) -> TaskDecision {
        let stamp = self.stamp();
        let plan = plan_withdraw(
            &stamp,
            &self.slice(),
            &self.task,
            &self.decisions.clone(),
            decision,
            reason,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        self.apply(plan)
    }

    /// Writes a plan the way the store does and returns the decision row it carries.
    fn apply(&mut self, plan: DecisionPlan) -> TaskDecision {
        let row = plan.decision.clone();
        match self.decisions.iter_mut().find(|d| d.id == row.id) {
            Some(slot) => *slot = row.clone(),
            None => self.decisions.push(row.clone()),
        }
        if let Some(comment) = plan.comment {
            self.comments.push(comment);
        }
        self.events.extend(plan.events);
        if let Some(update) = plan.tags {
            self.tags = update.tags.clone().unwrap_or_else(|| self.tags.clone());
            self.task = update.task;
            self.task_updates += 1;
        }
        row
    }

    fn tag_set(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        tags.sort_unstable();
        tags
    }

    fn bodies(&self) -> Vec<&str> {
        self.comments.iter().map(|c| c.body.as_str()).collect()
    }

    fn event_types(&self) -> Vec<&'static str> {
        self.events.iter().map(|e| e.event_type).collect()
    }
}

// ------------------------------------------------------------------ request

#[test]
fn request_tags_task_and_fills_option_ids() {
    let mut world = World::new(&[]);
    let d = world.request("Which store?", &options());
    assert_eq!(d.status, "open");
    assert_eq!(d.version, 1);
    assert_eq!(d.created_by, ACTOR);
    let ids: Vec<&str> = d.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    let recommended: Vec<bool> = d.options.iter().map(|o| o.recommended).collect();
    assert_eq!(recommended, [true, false]);
    assert!(world.tag_set().contains(&"needs-decision"));
    assert_eq!(world.task_updates, 1);
    assert_eq!(world.event_types(), ["task.decision_requested"]);
    assert!(world.comments.is_empty(), "request writes no comment");
}

#[test]
fn request_without_options_and_with_explicit_ids() {
    let mut world = World::new(&[]);
    let free = world.request("What should we call it?", &[]);
    assert!(free.options.is_empty());
    let named = world.request(
        "Which?",
        &[
            OptionInput {
                id: Some("sqlite".to_owned()),
                label: "SQLite".to_owned(),
                ..OptionInput::default()
            },
            opt("Other", false),
        ],
    );
    let ids: Vec<&str> = named.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, ["sqlite", "b"]);
}

#[test]
fn request_refuses_duplicate_option_ids() {
    let mut world = World::new(&[]);
    let dup = [
        OptionInput {
            id: Some("x".to_owned()),
            label: "1".to_owned(),
            ..OptionInput::default()
        },
        OptionInput {
            id: Some("x".to_owned()),
            label: "2".to_owned(),
            ..OptionInput::default()
        },
    ];
    let stamp = world.stamp();
    let result = plan_request(&stamp, &world.slice(), &world.task, "Dup?", &dup);
    assert_eq!(code_of(result), ErrorCode::ValidationFailed);
}

#[test]
fn request_refuses_an_empty_question_and_more_than_26_options() {
    let mut world = World::new(&[]);
    let stamp = world.stamp();
    let empty = plan_request(&stamp, &world.slice(), &world.task, "   ", &[]);
    assert_eq!(code_of(empty), ErrorCode::ValidationFailed);
    let many: Vec<OptionInput> = (0..27)
        .map(|n| opt(&format!("option {n}"), false))
        .collect();
    let result = plan_request(&stamp, &world.slice(), &world.task, "Many?", &many);
    assert_eq!(code_of(result), ErrorCode::ValidationFailed);
}

#[test]
fn request_keeps_an_existing_needs_decision_tag() {
    let mut world = World::new(&["needs-decision"]);
    let stamp = world.stamp();
    let plan = plan_request(&stamp, &world.slice(), &world.task, "Again?", &[])
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        plan.tags.is_none(),
        "no second tag update when the tag is present"
    );
}

// ------------------------------------------------------------------ choose

#[test]
fn choose_refuses_bad_answers_and_leaves_the_decision_open() {
    let mut world = World::new(&[]);
    let d = world.request("Which store?", &options());
    let unknown = world.try_choose(
        &d,
        Answer {
            option_id: Some("nope"),
            ..Answer::default()
        },
    );
    assert_eq!(code_of(unknown), ErrorCode::ValidationFailed);
    let neither = world.try_choose(&d, Answer::default());
    assert_eq!(code_of(neither), ErrorCode::ValidationFailed);
    let both = world.try_choose(
        &d,
        Answer {
            option_id: Some("a"),
            other_text: Some("both"),
            note: None,
        },
    );
    assert_eq!(code_of(both), ErrorCode::ValidationFailed);
    let blank = world.try_choose(
        &d,
        Answer {
            other_text: Some("  "),
            ..Answer::default()
        },
    );
    assert_eq!(code_of(blank), ErrorCode::ValidationFailed);
    assert!(world.tag_set().contains(&"needs-decision"));
    assert_eq!(world.decisions[0].status, "open");
}

#[test]
fn choose_on_a_task_without_options_refuses_an_option_id() {
    let mut world = World::new(&[]);
    let d = world.request("Free text?", &[]);
    let result = world.try_choose(
        &d,
        Answer {
            option_id: Some("a"),
            ..Answer::default()
        },
    );
    let error = match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed);
    assert!(
        error
            .message
            .ends_with("options are: none, answer with other_text")
    );
}

#[test]
fn choosing_the_last_open_decision_releases_the_task() {
    let mut world = World::new(&["parked", "triage:needs-decision", "keep"]);
    let d = world.request("Which store?", &options());
    let chosen = world.choose(
        &d,
        Answer {
            option_id: Some("b"),
            note: Some("  ops already runs one  "),
            ..Answer::default()
        },
    );
    assert_eq!(chosen.status, "decided");
    assert_eq!(chosen.chosen_option_id.as_deref(), Some("b"));
    assert_eq!(chosen.decided_by.as_deref(), Some(ACTOR));
    assert!(chosen.decided_at.is_some());
    assert_eq!(chosen.version, 2);
    assert_eq!(world.tag_set(), ["keep"]);
    let body = world.bodies().last().copied().unwrap_or_default();
    assert_eq!(
        body,
        "User decision 2026-01-01: Which store? -> Use Postgres\n\nNote: ops already runs one"
    );
    assert_eq!(
        world.event_types(),
        [
            "task.decision_requested",
            "task.comment_added",
            "task.decision_chosen"
        ]
    );
}

#[test]
fn choosing_with_another_open_decision_keeps_the_tags() {
    let mut world = World::new(&["parked"]);
    let first = world.request("First?", &options());
    let second = world.request("Second?", &options());
    world.choose(
        &first,
        Answer {
            option_id: Some("a"),
            ..Answer::default()
        },
    );
    assert!(world.tag_set().contains(&"needs-decision"));
    assert!(world.tag_set().contains(&"parked"));
    world.choose(
        &second,
        Answer {
            other_text: Some("Neither: use files"),
            ..Answer::default()
        },
    );
    assert!(world.tag_set().is_empty());
    assert!(
        world
            .bodies()
            .last()
            .is_some_and(|b| b.ends_with("Second? -> Neither: use files"))
    );
    let stored = world
        .decisions
        .iter()
        .find(|d| d.id == second.id)
        .unwrap_or_else(|| panic!("second decision missing"));
    assert_eq!(stored.chosen_option_id, None);
    assert_eq!(stored.other_text.as_deref(), Some("Neither: use files"));
}

#[test]
fn changing_an_answer_writes_a_new_comment_and_keeps_the_task_untouched() {
    let mut world = World::new(&[]);
    let d = world.request("Which store?", &options());
    let first = world.choose(
        &d,
        Answer {
            option_id: Some("a"),
            ..Answer::default()
        },
    );
    assert_eq!(first.version, 2);
    let again = world.choose(
        &first,
        Answer {
            other_text: Some("Actually, neither"),
            ..Answer::default()
        },
    );
    assert_eq!(again.status, "decided");
    assert_eq!(again.chosen_option_id, None);
    assert_eq!(again.other_text.as_deref(), Some("Actually, neither"));
    assert_eq!(again.version, 3);
    let answers = world
        .bodies()
        .iter()
        .filter(|b| b.starts_with("User decision"))
        .count();
    assert_eq!(answers, 2);
    assert!(!world.tag_set().contains(&"needs-decision"));
}

#[test]
fn choose_plan_writes_the_comment_before_the_events() {
    let mut world = World::new(&[]);
    let d = world.request("Which store?", &options());
    let stamp = world.stamp();
    let plan = plan_choose(
        &stamp,
        &world.slice(),
        &world.task,
        &world.decisions.clone(),
        &d,
        Answer {
            option_id: Some("a"),
            ..Answer::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(plan.comment.is_some());
    let types: Vec<&str> = plan.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, ["task.comment_added", "task.decision_chosen"]);
    let update = plan
        .tags
        .unwrap_or_else(|| panic!("the release drops tags"));
    assert!(
        update.event.is_some(),
        "the tag change is a task.updated event"
    );
}

// ------------------------------------------------------------------ withdraw

#[test]
fn withdraw_keeps_tags_until_the_last_open_decision_goes() {
    let mut world = World::new(&[]);
    let first = world.request("First?", &options());
    let second = world.request("Second?", &options());
    world.withdraw(&first, Some("no longer relevant"));
    assert!(world.tag_set().contains(&"needs-decision"));
    let gone = world.withdraw(&second, None);
    assert_eq!(gone.status, "withdrawn");
    assert_eq!(gone.chosen_option_id, None);
    assert!(world.tag_set().is_empty());
    assert_eq!(
        world.bodies()[0],
        "Decision withdrawn: First?\n\nReason: no longer relevant"
    );
    assert!(
        !world
            .bodies()
            .iter()
            .any(|b| b.starts_with("User decision"))
    );
}

#[test]
fn withdrawn_decisions_cannot_be_withdrawn_or_chosen_again() {
    let mut world = World::new(&[]);
    let first = world.request("First?", &options());
    let withdrawn = world.withdraw(&first, None);
    let stamp = world.stamp();
    let again = plan_withdraw(
        &stamp,
        &world.slice(),
        &world.task,
        &world.decisions.clone(),
        &withdrawn,
        None,
    );
    assert_eq!(code_of(again), ErrorCode::ValidationFailed);
    let chosen = world.try_choose(
        &withdrawn,
        Answer {
            option_id: Some("a"),
            ..Answer::default()
        },
    );
    assert_eq!(code_of(chosen), ErrorCode::ValidationFailed);
}

#[test]
fn an_empty_withdraw_reason_is_not_recorded() {
    let mut world = World::new(&[]);
    let d = world.request("Q?", &options());
    world.withdraw(&d, Some(""));
    assert_eq!(world.bodies(), ["Decision withdrawn: Q?"]);
}

#[test]
fn withdraw_emits_the_withdrawn_event() {
    let mut world = World::new(&[]);
    let d = world.request("Q?", &options());
    world.withdraw(&d, None);
    assert_eq!(
        world.event_types(),
        [
            "task.decision_requested",
            "task.comment_added",
            "task.decision_withdrawn"
        ]
    );
    let withdrawn = world.events.last().unwrap_or_else(|| panic!("no event"));
    assert_eq!(withdrawn.data["question"], "Q?");
    assert_eq!(withdrawn.data["decision_id"], d.id.to_string());
    assert_eq!(withdrawn.data["version"], 2);
}

// ------------------------------------------------------------------ normalisation

#[test]
fn option_ids_are_lettered_by_position_and_empty_ids_are_filled() {
    let out = wg_tasks::decisions::normalize_options(&[
        OptionInput {
            id: Some(String::new()),
            label: "first".to_owned(),
            ..OptionInput::default()
        },
        opt("second", false),
        opt("third", false),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    let ids: Vec<&str> = out.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
}
