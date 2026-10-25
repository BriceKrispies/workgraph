//! The minimal child environment.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

/// The variable names (compared ASCII case-insensitively) a child inherits by default. Secrets and
/// anything not listed are dropped.
pub const SAFE_ENV_KEYS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "LANG",
    "LC_ALL",
    "TERM",
    "USER",
    "USERNAME",
    "LOGNAME",
    "SHELL",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "VIRTUAL_ENV",
    "UV_CACHE_DIR",
    "PYTHONUTF8",
    "HOMEDRIVE",
    "HOMEPATH",
];

/// The minimal environment built from an explicit parent environment: only [`SAFE_ENV_KEYS`] are
/// kept (unless `inherit_all`), then `extra` is applied on top.
///
/// On Windows variable names are case-insensitive: an `extra` entry replaces an inherited one
/// whatever its case.
pub fn base_environment_from<P, I, K, V>(
    parent: P,
    extra: I,
    inherit_all: bool,
) -> BTreeMap<OsString, OsString>
where
    P: IntoIterator<Item = (OsString, OsString)>,
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env: BTreeMap<OsString, OsString> = parent
        .into_iter()
        .filter(|(key, _)| inherit_all || is_safe(key))
        .collect();
    for (key, value) in extra {
        let key = key.as_ref();
        if cfg!(windows) {
            env.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        }
        env.insert(key.to_os_string(), value.as_ref().to_os_string());
    }
    env
}

fn is_safe(key: &OsStr) -> bool {
    key.to_str().is_some_and(|k| {
        SAFE_ENV_KEYS
            .iter()
            .any(|safe| safe.eq_ignore_ascii_case(k))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        items
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn drops_secrets_and_keeps_safe_keys_case_insensitively() {
        let parent = pairs(&[
            ("Path", "/bin"),
            ("API_TOKEN", "x"),
            ("home", "/h"),
            ("LANG", "C"),
        ]);
        let env = base_environment_from(parent, Vec::<(&str, &str)>::new(), false);
        let keys: Vec<_> = env.keys().map(|k| k.to_str().unwrap_or("")).collect();
        assert_eq!(keys, ["LANG", "Path", "home"]);
    }

    #[test]
    fn inherit_all_keeps_everything() {
        let parent = pairs(&[("API_TOKEN", "x"), ("PATH", "p")]);
        let env = base_environment_from(parent, [("A", "1")], true);
        assert_eq!(env.len(), 3);
        assert_eq!(env.get(OsStr::new("API_TOKEN")), Some(&OsString::from("x")));
    }

    #[test]
    fn extra_overrides_inherited() {
        let parent = pairs(&[("PATH", "old")]);
        let env = base_environment_from(parent, [("PATH", "new")], false);
        assert_eq!(env.get(OsStr::new("PATH")), Some(&OsString::from("new")));
        assert_eq!(env.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn extra_replaces_regardless_of_case_on_windows() {
        let parent = pairs(&[("Path", "old")]);
        let env = base_environment_from(parent, [("PATH", "new")], false);
        assert_eq!(env.len(), 1);
        assert_eq!(env.get(OsStr::new("PATH")), Some(&OsString::from("new")));
    }
}
