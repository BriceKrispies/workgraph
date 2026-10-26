//! The in-process signals behind the `Wake` and `Publish` effects.
//!
//! * Wakes: a counter and a condition variable. A local worker idle-waiting for work remembers the
//!   counter it last saw and waits until it moves (or its poll interval passes), so a decision that
//!   enqueued a job starts it without waiting for the next poll.
//! * Stream events: per-subscriber channels, filtered by workspace, for the SSE endpoints of this
//!   process. A subscriber that went away is dropped on the next publish.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use wg_model::{JobWake, StreamEvent};
use wg_types::Id;

/// The process-wide signals. Clones share them.
#[derive(Debug, Clone, Default)]
pub struct Bus {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    wakes: Mutex<u64>,
    woken: Condvar,
    subscribers: Mutex<Vec<(Id, Sender<StreamEvent>)>>,
}

/// Locks a mutex; a panic while it was held does not poison the signals.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Bus {
    /// Performs a `Wake` effect: bumps the counter and wakes every waiter.
    pub fn wake(&self, _wake: &JobWake) {
        let mut count = lock(&self.inner.wakes);
        *count = count.wrapping_add(1);
        self.inner.woken.notify_all();
    }

    /// The number of wakes so far.
    pub fn wakes(&self) -> u64 {
        *lock(&self.inner.wakes)
    }

    /// Waits until the wake counter differs from `seen` or `timeout` passes; returns the counter.
    pub fn wait_for_wake(&self, seen: u64, timeout: Duration) -> u64 {
        let guard = lock(&self.inner.wakes);
        let (guard, _) = self
            .inner
            .woken
            .wait_timeout_while(guard, timeout, |count| *count == seen)
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }

    /// A stream of the events published for `workspace_id` from now on.
    pub fn subscribe(&self, workspace_id: Id) -> Receiver<StreamEvent> {
        let (sender, receiver) = channel();
        lock(&self.inner.subscribers).push((workspace_id, sender));
        receiver
    }

    /// Performs a `Publish` effect.
    pub fn publish(&self, event: &StreamEvent) {
        lock(&self.inner.subscribers).retain(|(workspace_id, sender)| {
            *workspace_id != event.workspace_id || sender.send(event.clone()).is_ok()
        });
    }
}
