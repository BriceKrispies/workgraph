//! wg-events: event records, schema versions, upcasters, projection (CF-06, O13, C067).
//!
//! The record is [`wg_model::StoredEvent`]. This crate holds what reads a record back:
//!
//! * [`upcast`] converts a payload written under an older schema to the current version of its
//!   type, on read (`core/events/log.py` `upcast` and `to_record`);
//! * [`projection`] replays the task events into the task state they describe
//!   (`core/events/projection.py`);
//! * [`verify`] compares that replay with the current-state tables (`events verify`, C067).
//!
//! Everything here is a pure function of its inputs, `Core` tier (plan 4.6): the shell reads the
//! events and the task rows, and passes them in.

pub mod projection;
pub mod upcast;
pub mod verify;

pub use projection::{ProjectedTask, ProjectionError, project_tasks};
pub use upcast::{current_version, read, upcast};
pub use verify::{Mismatch, TaskState, VerifyReport, task_states, verify_task_projection};
