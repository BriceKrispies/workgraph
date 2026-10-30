//! The load-decide-apply cycle (section 4.1) and the flow driver (section 4.2).
//!
//! ```text
//!   BEGIN IMMEDIATE
//!   load(session) -> slice            the shell reads what the decision needs
//!   stamp                             clock and entropy, once per decision
//!   decide(&slice, &stamp) -> Decision
//!   apply writes + events, COMMIT
//!   perform effects                   outside any transaction
//! ```
//!
//! Reads, the load and the decision run in the same transaction as the writes, as the Python
//! services do: the decision is pure and fast, so holding the write lock while it runs costs
//! nothing, and there is no retry loop. A decision that fails rolls everything back and performs
//! no effect.
//!
//! A flow ([`Flow`]) is a state machine in a `Core` crate: each step is one decision, and the
//! outcome of each effect it asked for comes back to it as the next input.

use std::collections::VecDeque;

use crate::spec::RequestContext;
use wg_model::{Decision, EffectResult, Stamp};
use wg_store::{Applied, Session, Store};
use wg_types::WgError;

use crate::effects::Interpreter;
use crate::stamp::StampSource;

/// The runtime of one thread: its database connection, where its stamps come from and how its
/// effects are performed. Every interface (CLI, HTTP, MCP, workers) calls into one.
pub struct Runtime {
    pub store: Store,
    pub stamps: Box<dyn StampSource>,
    pub effects: Interpreter,
}

/// What one decision did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome<R> {
    pub reply: R,
    /// The stamp the decision was made with.
    pub stamp: Stamp,
    /// The `seq` of each appended event.
    pub applied: Applied,
    /// The outcome of each effect, in the decision's order.
    pub effects: Vec<EffectResult>,
}

/// What one flow step returns: the next state and the decision to apply.
pub type Step<St> = Result<(St, Decision<()>), WgError>;

/// A long-running flow as plain functions, so a `Core` crate's state machine plugs in directly.
///
/// * `load` reads the slice a step needs, in the step's transaction. Flow state that must survive
///   a crash lives in rows (attempt, stack and job status), so `load` reads it from there too.
/// * `step` is the pure transition: one decision per input.
/// * `input` turns an effect's outcome into the flow's next input.
/// * `finished` says when the flow is done; inputs left over are dropped.
pub struct Flow<St, In, Sl> {
    /// Names the flow in logs.
    pub name: &'static str,
    pub load: fn(&mut Session<'_>, &St) -> Result<Sl, WgError>,
    pub step: fn(St, &In, &Sl, &Stamp) -> Step<St>,
    pub input: fn(EffectResult) -> In,
    pub finished: fn(&St) -> bool,
}

impl Runtime {
    /// Runs one decision: `load` and `decide` inside a `BEGIN IMMEDIATE` transaction, the
    /// decision's writes and events applied and committed, then its effects performed.
    pub fn decide<S, R>(
        &mut self,
        context: &RequestContext,
        load: impl FnOnce(&mut Session<'_>) -> Result<S, WgError>,
        decide: impl FnOnce(&S, &Stamp) -> Result<Decision<R>, WgError>,
    ) -> Result<Outcome<R>, WgError> {
        self.decide_then(context, load, decide, |_, _| Ok(()))
    }

    /// [`decide`](Self::decide) with a step that runs in the same transaction after the
    /// decision's writes and events are applied (index maintenance, for one).
    pub fn decide_then<S, R>(
        &mut self,
        context: &RequestContext,
        load: impl FnOnce(&mut Session<'_>) -> Result<S, WgError>,
        decide: impl FnOnce(&S, &Stamp) -> Result<Decision<R>, WgError>,
        after: impl FnOnce(&mut Session<'_>, &Decision<R>) -> Result<(), WgError>,
    ) -> Result<Outcome<R>, WgError> {
        let stamps = &mut self.stamps;
        let (decision, stamp, applied) = self.store.write(|session| {
            let slice = load(session)?;
            let stamp = stamps.stamp(context)?;
            let decision = decide(&slice, &stamp)?;
            let applied = session.apply_decision(&decision)?;
            after(session, &decision)?;
            Ok((decision, stamp, applied))
        })?;
        tracing::debug!(
            target: "workgraph.runtime",
            actor = %stamp.actor,
            correlation_id = %stamp.correlation,
            writes = decision.writes.len(),
            events = decision.events.len(),
            effects = decision.effects.len(),
            "decision applied"
        );
        let effects = self.effects.perform_all(&decision.effects);
        Ok(Outcome {
            reply: decision.reply,
            stamp,
            applied,
            effects,
        })
    }

    /// A read: `load` in a read transaction (a consistent snapshot), then the pure `answer`.
    pub fn query<S, T>(
        &mut self,
        load: impl FnOnce(&mut Session<'_>) -> Result<S, WgError>,
        answer: impl FnOnce(&S) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let slice = self.store.read(load)?;
        answer(&slice)
    }

    /// Drives `flow` from `state` with `first` as its first input until it is finished or no input
    /// is left. Each step is one [`decide`](Self::decide)-shaped transaction; the outcomes of the
    /// step's effects are queued, in order, as the next inputs. Returns the last state.
    pub fn drive<St, In, Sl>(
        &mut self,
        flow: &Flow<St, In, Sl>,
        context: &RequestContext,
        state: St,
        first: In,
    ) -> Result<St, WgError> {
        let mut pending = VecDeque::from([first]);
        let mut state = state;
        while let Some(input) = pending.pop_front() {
            if (flow.finished)(&state) {
                break;
            }
            let stamps = &mut self.stamps;
            let (next, decision) = self.store.write(|session| {
                let slice = (flow.load)(session, &state)?;
                let stamp = stamps.stamp(context)?;
                let (next, decision) = (flow.step)(state, &input, &slice, &stamp)?;
                session.apply_decision(&decision)?;
                Ok((next, decision))
            })?;
            state = next;
            tracing::debug!(
                target: "workgraph.runtime.flow",
                flow = flow.name,
                writes = decision.writes.len(),
                events = decision.events.len(),
                effects = decision.effects.len(),
                "flow step applied"
            );
            pending.extend(
                self.effects
                    .perform_all(&decision.effects)
                    .into_iter()
                    .map(flow.input),
            );
        }
        Ok(state)
    }
}
