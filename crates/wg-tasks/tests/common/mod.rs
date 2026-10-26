//! Builders for slices used by the GV-19 vector test and the CORE-02 unit tests.

#![allow(dead_code)]

use rpds::VectorSync;
use uuid::Uuid;
use wg_model::entity::{Stack, StackItem, Task, TaskDependency, TaskTag};
use wg_model::slice::StackSlice;
use wg_model::{Row, TaskSlice, Write};
use wg_types::{Id, StackItemState, StackStatus, TaskKey, TaskPriority, TaskStatus, Timestamp};

pub const WORKSPACE: u128 = 0xFFFF;

pub fn id(n: u128) -> Id {
    Uuid::from_u128(n)
}

pub fn workspace() -> Id {
    id(WORKSPACE)
}

/// `seconds` (under a day) after 2026-01-01T00:00:00Z.
pub fn stamp(seconds: i64) -> Timestamp {
    let s = u32::try_from(seconds).unwrap_or(0);
    Timestamp::from_components(
        2026,
        1,
        1,
        u8::try_from(s / 3600).unwrap_or(0),
        u8::try_from(s / 60 % 60).unwrap_or(0),
        u8::try_from(s % 60).unwrap_or(0),
        0,
    )
    .unwrap_or_else(|e| panic!("timestamp: {e}"))
}

/// A task numbered `n` (id `n`, key `<prefix>-<n>`), created `n` seconds after the epoch.
pub fn task(prefix: &str, n: i64, status: TaskStatus, parent: Option<Id>) -> Task {
    let at = stamp(n);
    Task {
        id: id(u128::try_from(n).unwrap_or(0)),
        workspace_id: workspace(),
        number: n,
        key: TaskKey::parse(&format!("{prefix}-{n}")).unwrap_or_else(|e| panic!("{e}")),
        title: format!("Task {n}"),
        description: String::new(),
        description_hash: String::new(),
        acceptance_criteria: VectorSync::new_sync(),
        status,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: parent,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: serde_json::Map::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "system".to_owned(),
        version: 1,
        created_at: at,
        updated_at: at,
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

pub fn tag(task_id: Id, name: &str) -> TaskTag {
    TaskTag {
        task_id,
        tag: name.to_owned(),
    }
}

/// The dependency `task_id` depends on `depends_on_id`.
pub fn dependency(seq: u128, task_id: Id, depends_on_id: Id) -> TaskDependency {
    TaskDependency {
        id: id(0x1_0000 + seq),
        task_id,
        depends_on_id,
        created_by: "system".to_owned(),
        created_at: stamp(0),
    }
}

pub fn tasks_slice(tasks: Vec<Task>, tags: Vec<TaskTag>, deps: Vec<(Id, Id)>) -> TaskSlice {
    let writes: Vec<Write> = tasks
        .into_iter()
        .map(|t| Write::Insert(Row::Task(t)))
        .chain(tags.into_iter().map(|t| Write::Insert(Row::TaskTag(t))))
        .chain(
            deps.into_iter()
                .enumerate()
                .map(|(i, (t, d))| Write::Insert(Row::TaskDependency(dependency(i as u128, t, d)))),
        )
        .collect();
    TaskSlice::default().apply_all(&writes)
}

/// A stack of `parent` with one item per `(task id, state)`.
/// A stack of a parent task: its status and its items as `(task id, state)`.
pub type StackSpec = (Id, StackStatus, Vec<(Id, StackItemState)>);

pub fn stack_slice(stacks: Vec<StackSpec>) -> StackSlice {
    let writes: Vec<Write> = stacks
        .into_iter()
        .enumerate()
        .flat_map(|(i, (parent, status, items))| {
            let stack_id = id(0x2_0000 + i as u128);
            let stack = Stack {
                id: stack_id,
                workspace_id: workspace(),
                parent_task_id: parent,
                status,
                created_at: stamp(0),
                updated_at: stamp(0),
                land_request: None,
            };
            std::iter::once(Write::Insert(Row::Stack(stack))).chain(
                items
                    .into_iter()
                    .enumerate()
                    .map(move |(j, (task_id, state))| {
                        Write::Insert(Row::StackItem(StackItem {
                            id: id(0x3_0000 + (i as u128) * 1000 + j as u128),
                            stack_id,
                            task_id,
                            state,
                            merge_order: None,
                            detail: None,
                            created_at: stamp(0),
                            updated_at: stamp(0),
                        }))
                    }),
            )
        })
        .collect();
    StackSlice::default().apply_all(&writes)
}
