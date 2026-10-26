//! The task commands as pure functions of the task world (IF-05): `task create`, `list`,
//! `show`, `update`, `bulk-update`, `archive`, `graph`, `ready` and `promote-ready`.
//!
//! * [`requests`]: the request models and the typed values read from a validated request;
//! * [`out`]: `TaskOut`, `TaskDetail` and the values inside them, assembled from a slice;
//! * [`reads`]: list, ready queue, detail, graph and the event value of `history`;
//! * [`writes`]: create, update, archive and promote, as decisions.
//!
//! The runtime loads a [`TaskWorld`](wg_storecodec::task_world_sql::TaskWorld), calls one of
//! these and applies the decision it returns.

pub mod out;
pub mod reads;
pub mod requests;
pub mod writes;
