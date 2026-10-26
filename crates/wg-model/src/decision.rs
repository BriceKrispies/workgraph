//! [`Decision`]: what one pure decision asks the shell to do.

use rpds::VectorSync;

use crate::effect::Effect;
use crate::event::EventDraft;
use crate::write::Write;

/// The result of a decision: rows to write and events to append in one transaction, effects to
/// perform after it commits, and the reply to render.
///
/// Built by value: `Decision::new(reply).write(w).event(e)`. A helper that contributes changes but
/// no reply returns `Decision<()>`, and the caller joins it with [`Decision::then`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision<R> {
    /// Applied in order, inside the transaction.
    pub writes: VectorSync<Write>,
    /// Appended to `events` in order, in the same transaction.
    pub events: VectorSync<EventDraft>,
    /// Performed after commit; results come back as flow inputs.
    pub effects: VectorSync<Effect>,
    pub reply: R,
}

impl<R> Decision<R> {
    /// A decision that changes nothing and replies `reply`.
    pub fn new(reply: R) -> Self {
        Decision {
            writes: VectorSync::new_sync(),
            events: VectorSync::new_sync(),
            effects: VectorSync::new_sync(),
            reply,
        }
    }

    #[must_use]
    pub fn write(self, write: Write) -> Self {
        Decision {
            writes: self.writes.push_back(write),
            ..self
        }
    }

    #[must_use]
    pub fn event(self, event: EventDraft) -> Self {
        Decision {
            events: self.events.push_back(event),
            ..self
        }
    }

    #[must_use]
    pub fn effect(self, effect: Effect) -> Self {
        Decision {
            effects: self.effects.push_back(effect),
            ..self
        }
    }

    /// Replaces the reply, keeping the changes.
    pub fn with_reply<S>(self, reply: S) -> Decision<S> {
        Decision {
            writes: self.writes,
            events: self.events,
            effects: self.effects,
            reply,
        }
    }

    /// Transforms the reply, keeping the changes.
    pub fn map<S>(self, f: impl FnOnce(R) -> S) -> Decision<S> {
        Decision {
            writes: self.writes,
            events: self.events,
            effects: self.effects,
            reply: f(self.reply),
        }
    }

    /// This decision's changes followed by `next`'s, with `next`'s reply.
    pub fn then<S>(self, next: Decision<S>) -> Decision<S> {
        Decision {
            writes: next
                .writes
                .iter()
                .fold(self.writes, |acc, write| acc.push_back(write.clone())),
            events: next
                .events
                .iter()
                .fold(self.events, |acc, event| acc.push_back(event.clone())),
            effects: next
                .effects
                .iter()
                .fold(self.effects, |acc, effect| acc.push_back(effect.clone())),
            reply: next.reply,
        }
    }

    /// Whether the decision changes nothing (no writes, events or effects).
    pub fn is_noop(&self) -> bool {
        self.writes.is_empty() && self.events.is_empty() && self.effects.is_empty()
    }
}
