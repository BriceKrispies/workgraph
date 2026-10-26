//! Stamping: the clock reading and the entropy of each decision (section 4.5).
//!
//! The clock is read once and 32 bytes are drawn from the OS once per decision; a
//! [`RequestContext`] turns them into the decision's [`Stamp`]. Tests and replays plug their own
//! [`StampSource`] into the runtime to make decisions reproducible.

use wg_model::{Stamp, derive_id};
use wg_runtimespec::{GETUSER_VARIABLES, RequestContext, getuser, timestamp_from_unix_nanos};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

/// Where the runtime gets each decision's stamp.
pub trait StampSource: Send {
    /// The stamp of the next decision made for `context`.
    fn stamp(&mut self, context: &RequestContext) -> Result<Stamp, WgError>;
}

/// The production source: the system clock and the OS random number generator.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemStamps;

impl StampSource for SystemStamps {
    fn stamp(&mut self, context: &RequestContext) -> Result<Stamp, WgError> {
        let (at, entropy) = draw()?;
        Ok(context.stamp(at, entropy))
    }
}

/// Reads the clock (truncated to microseconds) and draws 32 bytes from the OS.
pub fn draw() -> Result<(Timestamp, [u8; 32]), WgError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| WgError::new(ErrorCode::WorkgraphError, format!("clock error: {err}")))?
        .as_nanos();
    let at = timestamp_from_unix_nanos(i128::try_from(nanos).unwrap_or(i128::MAX))?;
    let mut entropy = [0_u8; 32];
    getrandom::fill(&mut entropy).map_err(|err| {
        WgError::new(
            ErrorCode::WorkgraphError,
            format!("cannot draw entropy from the OS: {err}"),
        )
    })?;
    Ok((at, entropy))
}

/// `new_id()`: a fresh random id (a request's correlation id, for one).
pub fn new_id() -> Result<Id, WgError> {
    let (_, entropy) = draw()?;
    Ok(derive_id(&entropy, 0))
}

/// `RequestContext.human(name)`, with the login name read as `getpass.getuser()` reads it.
pub fn human_context(name: Option<&str>) -> Result<RequestContext, WgError> {
    let values: Vec<Option<String>> = GETUSER_VARIABLES
        .iter()
        .map(|key| std::env::var(key).ok())
        .collect();
    RequestContext::human(name, getuser(&values), new_id()?)
}

/// `RequestContext.system(component)` with a fresh correlation id.
pub fn system_context(component: &str) -> Result<RequestContext, WgError> {
    let (_, entropy) = draw()?;
    RequestContext::system(component, derive_id(&entropy, 0))
}

/// `RequestContext.agent(name, permissions)` with a fresh correlation id.
pub fn agent_context(
    name: &str,
    permissions: Option<std::collections::BTreeSet<wg_types::Permission>>,
) -> Result<RequestContext, WgError> {
    let (_, entropy) = draw()?;
    RequestContext::agent(name, permissions, derive_id(&entropy, 0))
}
