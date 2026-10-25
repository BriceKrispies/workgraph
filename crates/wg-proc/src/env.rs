//! The minimal child environment from this process's environment.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

/// The minimal environment built from this process's environment: only
/// [`SAFE_ENV_KEYS`](crate::SAFE_ENV_KEYS) are kept (unless `inherit_all`), then `extra` is
/// applied on top. See [`base_environment_from`](crate::base_environment_from).
pub fn base_environment<I, K, V>(extra: I, inherit_all: bool) -> BTreeMap<OsString, OsString>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    wg_procspec::base_environment_from(std::env::vars_os(), extra, inherit_all)
}
