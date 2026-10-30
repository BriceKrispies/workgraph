//! The task commands as pure functions of the task world (IF-05): `task create`, `list`,
//! `show`, `update`, `bulk-update`, `archive`, `graph`, `ready`, `promote-ready`, `accept`,
//! `reject`, claims and links.
//!
//! * [`requests`]: the request models and the typed values read from a validated request;
//! * [`out`]: `TaskOut`, `TaskDetail` and the values inside them, assembled from a slice;
//! * [`reads`]: list, ready queue, detail, graph and the event value of `history`;
//! * [`writes`]: create, update, archive and promote, as decisions;
//! * [`claims`], [`links`], [`bodies`], [`query`]: the claim lifecycle, the links and the request
//!   bodies of the remaining commands;
//! * [`acceptance`]: accept, reject and promote-dependents over the task world.
//!
//! The runtime loads a [`TaskWorld`](wg_rows::task_world_sql::TaskWorld), calls one of
//! these and applies the decision it returns.

pub mod acceptance;
pub mod args;
pub mod bodies;
pub mod claims;
pub mod contract;
pub mod links;
pub mod query;
pub mod reads;
pub mod requests;
pub mod routes;
pub mod tool_args;
pub mod triage;
pub mod view;
pub mod writes;
