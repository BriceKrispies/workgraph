//! wg-types: ids, task keys, timestamps, enums, actors, permissions, errors.
//!
//! Pure value types shared by every Core crate. No clock, no randomness, no I/O.

#[macro_use]
mod macros;

mod actor;
mod enums;
mod error;
mod id;
mod key;
mod permission;
mod repr;
mod time;

pub use actor::Actor;
pub use enums::*;
pub use error::{ErrorCode, WgError};
pub use id::{Correlation, Id, parse_id};
pub use key::TaskKey;
pub use permission::{
    Permission, all_permissions, default_agent_permissions, human_permissions, parse_permissions,
    read_permissions, system_permissions,
};
pub use repr::py_repr_str;
pub use time::Timestamp;
