//! [`Chain`]: the steps of one stack-executor transaction, applied one after another to the
//! slices the transaction loaded.
//!
//! Every function of the executor in `integrations/git/stack_*.py` is a few row changes in one
//! session: move an item, change a task's status, leave a comment, record an event. A chain keeps
//! the decision built so far, the slices after it and the ids still free, so each step sees what
//! the one before wrote (`session.flush()`) and the whole is one decision.

use serde_json::{Map, Value};
use wg_jobs::queue::NewJob;
use wg_model::entity::{Stack, StackItem, StackRepository};
use wg_model::event::{EventDraft, EventPayload, StackStatusChanged};
use wg_model::slice::QueueSlice;
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, Task, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Change, transition};
use wg_types::{CommentKind, Id, StackItemState, StackStatus, TaskStatus, WgError};

use crate::spec::writes::{Step, integration_jobs, repository_ids, transition_item};
use wg_tasks::accept::comment_step;

/// One transaction of the executor, being built.
#[derive(Clone)]
pub struct Chain<'a> {
    pub stamp: &'a Stamp,
    pub(crate) step: Step,
    /// Salts for the sub-stamps of jobs enqueued by this chain, so that their ids differ.
    salt: u32,
}

impl<'a> Chain<'a> {
    /// A chain over `world`.
    #[must_use]
    pub fn start(world: &TaskWorld, stamp: &'a Stamp) -> Self {
        Chain {
            stamp,
            step: Step {
                decision: Decision::new(()),
                tasks: world.tasks.clone(),
                stacks: world.stacks.clone(),
                ids: stamp.ids(),
            },
            salt: 300,
        }
    }

    /// The task as the chain has it now.
    pub fn task(&self, id: Id) -> Result<&Task, WgError> {
        self.step
            .tasks
            .task(id)
            .ok_or_else(|| WgError::workgraph(format!("task {} vanished from the slice", id)))
    }

    /// The stack as the chain has it now.
    pub fn stack(&self, id: Id) -> Result<&Stack, WgError> {
        self.step
            .stacks
            .stack(id)
            .ok_or_else(|| WgError::workgraph(format!("stack {} vanished from the slice", id)))
    }

    /// The item as the chain has it now.
    pub fn item(&self, id: Id) -> Result<&StackItem, WgError> {
        self.step
            .stacks
            .items
            .get(&id)
            .ok_or_else(|| WgError::workgraph(format!("stack item {} vanished from the slice", id)))
    }

    /// The stack's repository rows as the chain has them now.
    pub fn repositories(&self, stack_id: Id) -> Vec<&StackRepository> {
        self.step.stacks.repositories_of(stack_id).collect()
    }

    /// Takes the next id.
    #[must_use]
    pub fn next_id(self) -> (Id, Self) {
        let (id, ids) = self.step.ids.next();
        (
            id,
            Chain {
                step: Step { ids, ..self.step },
                ..self
            },
        )
    }

    fn apply(self, writes: Decision<()>, ids: IdSeq) -> Self {
        let stacks = self.step.stacks.apply_all(writes.writes.iter());
        let tasks = self.step.tasks.apply_all(writes.writes.iter());
        Chain {
            step: Step {
                decision: self.step.decision.then(writes),
                tasks,
                stacks,
                ids,
            },
            ..self
        }
    }

    /// Records an event about `entity` and returns its id.
    #[must_use]
    pub fn event_with_id(
        self,
        workspace: Id,
        entity: Id,
        payload: EventPayload,
        cause: Option<Id>,
    ) -> (Id, Self) {
        self.event_for_attempt(workspace, entity, payload, cause, None)
    }

    /// Records an event that names an attempt, and returns its id.
    #[must_use]
    pub fn event_for_attempt(
        self,
        workspace: Id,
        entity: Id,
        payload: EventPayload,
        cause: Option<Id>,
        attempt: Option<Id>,
    ) -> (Id, Self) {
        let (id, ids) = self.step.ids.next();
        let draft =
            EventDraft::new(self.stamp, id, Some(workspace), entity, payload).with_attempt(attempt);
        let draft = match cause {
            Some(cause) => draft.caused_by(cause),
            None => draft,
        };
        let chain = Chain {
            step: Step {
                decision: self.step.decision.event(draft),
                ids,
                ..self.step
            },
            ..self
        };
        (id, chain)
    }

    /// Records an event about `entity`.
    #[must_use]
    pub fn event(self, workspace: Id, entity: Id, payload: EventPayload) -> Self {
        self.event_with_id(workspace, entity, payload, None).1
    }

    /// Writes a row change (an insert, an update or a delete).
    #[must_use]
    pub fn write(self, write: Write) -> Self {
        let decision = Decision::new(()).write(write);
        let ids = self.step.ids;
        self.apply(decision, ids)
    }

    /// `StackService.transition_item`: moves a task's item of `stack_id` to `target`.
    pub fn item_to(
        self,
        stack_id: Id,
        task_id: Id,
        target: StackItemState,
        detail: Option<String>,
    ) -> Result<Self, WgError> {
        let stack = self.stack(stack_id)?.clone();
        let parent = self.task(stack.parent_task_id)?.clone();
        let child = self.task(task_id)?.clone();
        let item = self
            .step
            .stacks
            .items_of(stack_id)
            .find(|item| item.task_id == task_id)
            .cloned()
            .ok_or_else(|| {
                WgError::workgraph(format!(
                    "stack item of task {} vanished from the slice",
                    task_id
                ))
            })?;
        let next = transition_item(
            &self.step, self.stamp, &stack, &parent, &child, &item, target, detail,
        )?;
        Ok(Chain {
            step: Step {
                decision: self.step.decision.then(next.decision),
                tasks: next.tasks,
                stacks: next.stacks,
                ids: next.ids,
            },
            ..self
        })
    }

    /// `item.detail = ...; stack.updated_at = now`: sets an item's detail without a transition.
    #[must_use]
    pub fn item_detail(self, item_id: Id, detail: String) -> Self {
        match self.item(item_id).cloned() {
            Ok(item) => {
                let stack_id = item.stack_id;
                let moved = StackItem {
                    detail: Some(detail),
                    updated_at: self.stamp.at,
                    ..item
                };
                self.write(Write::update(moved)).touch_stack(stack_id)
            }
            Err(_) => self,
        }
    }

    /// `stack.updated_at = now`.
    #[must_use]
    pub fn touch_stack(self, stack_id: Id) -> Self {
        match self.stack(stack_id).cloned() {
            Ok(stack) => {
                let at = self.stamp.at;
                self.write(Write::update(Stack {
                    updated_at: at,
                    ..stack
                }))
            }
            Err(_) => self,
        }
    }

    /// `StackVerifier.set_status` and `StackIntegrator.start_integrating`: a status change with
    /// its event; none when the stack already has the status.
    #[must_use]
    pub fn stack_status(self, stack_id: Id, target: StackStatus, reason: &str) -> Self {
        let Ok(stack) = self.stack(stack_id).cloned() else {
            return self;
        };
        if stack.status == target {
            return self;
        }
        let workspace = stack.workspace_id;
        let previous = stack.status;
        let at = self.stamp.at;
        self.write(Write::update(Stack {
            status: target,
            updated_at: at,
            ..stack
        }))
        .event(
            workspace,
            stack_id,
            EventPayload::StackStatusChanged(StackStatusChanged {
                from: previous,
                to: target,
                reason: reason.to_owned(),
            }),
        )
    }

    /// `TaskService._transition`.
    pub fn task_to(
        self,
        task_id: Id,
        target: TaskStatus,
        change: Change<'_>,
    ) -> Result<Self, WgError> {
        let applied = transition(
            &self.step.tasks,
            &self.step.stacks,
            self.stamp,
            self.step.ids,
            task_id,
            target,
            change,
        )?;
        Ok(Chain {
            step: Step {
                decision: self.step.decision.then(applied.decision),
                tasks: applied.tasks,
                stacks: self.step.stacks,
                ids: applied.ids,
            },
            ..self
        })
    }

    /// `TaskService._comment` with the default kind and no attempt.
    pub fn comment(
        self,
        task_id: Id,
        body: &str,
        data: &Map<String, Value>,
        cause: Option<Id>,
    ) -> Result<Self, WgError> {
        let workspace = self.task(task_id)?.workspace_id;
        let made = comment_step(
            self.stamp,
            self.step.ids,
            task_id,
            workspace,
            CommentKind::Comment,
            body,
            data,
            None,
            cause,
        );
        Ok(self.apply(made.decision, made.ids))
    }

    /// `StackService.enqueue_integration`: a `stack_integrate` job per repository of the stack
    /// other than `exclude`, keyed by `version`; `queue` holds the jobs already under those keys.
    #[must_use]
    pub fn enqueue_integration(
        self,
        stack_id: Id,
        exclude: Option<Id>,
        version: Id,
        queue: &QueueSlice,
    ) -> Self {
        let Ok(stack) = self.stack(stack_id).cloned() else {
            return self;
        };
        let repositories: Vec<Id> = repository_ids(&self.step.stacks, stack_id)
            .into_iter()
            .filter(|id| Some(*id) != exclude)
            .collect();
        let jobs = integration_jobs(&stack, &repositories, version, self.stamp, queue);
        let ids = self.step.ids;
        jobs.into_iter().fold(self, |chain, (_, job)| {
            let writes = job.map(|_| ());
            chain.apply(writes, ids)
        })
    }

    /// Enqueues one job (`jobqueue.enqueue`): nothing when a stored job holds its idempotency key.
    #[must_use]
    pub fn enqueue(self, job: &NewJob, queue: &QueueSlice) -> Self {
        let salt = self.salt;
        let queued =
            wg_jobs::queue::enqueue(queue, job, &wg_tasks::accept::substamp(self.stamp, salt));
        let ids = self.step.ids;
        Chain {
            salt: salt + 1,
            ..self.apply(queued.map(|_| ()), ids)
        }
    }

    /// The chain as a decision with `reply`.
    #[must_use]
    pub fn finish<R>(self, reply: R) -> Decision<R> {
        self.step.decision.with_reply(reply)
    }
}
