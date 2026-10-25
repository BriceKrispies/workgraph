//! Executable resolution (`shutil.which` with `PATHEXT` on Windows).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use wg_procspec::{ProcError, executable_suffixes, validate_executable_name};

/// Resolve `name` using this process's `PATH` and `PATHEXT`.
///
/// # Errors
/// [`ProcError::InvalidExecutable`] for an empty name or one with a NUL byte;
/// [`ProcError::NotFound`] when nothing matches.
pub fn resolve_executable(name: &str) -> Result<PathBuf, ProcError> {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    resolve_executable_in(name, path.as_deref(), pathext.as_deref())
}

/// [`resolve_executable`] with an explicit `PATH` and `PATHEXT` (the latter used only on Windows).
///
/// A name with a directory part (or an absolute path) is checked directly, without a search. On
/// Windows a name without a known extension is tried with each `PATHEXT` extension in turn.
///
/// # Errors
/// As [`resolve_executable`].
pub fn resolve_executable_in(
    name: &str,
    path: Option<&OsStr>,
    pathext: Option<&OsStr>,
) -> Result<PathBuf, ProcError> {
    validate_executable_name(name)?;
    let candidate = Path::new(name);
    let has_dir = candidate.is_absolute() || candidate.components().count() > 1;
    let extension = candidate.extension().and_then(OsStr::to_str);
    let suffixes = executable_suffixes(extension, pathext.and_then(OsStr::to_str), cfg!(windows));
    if has_dir {
        return first_match(candidate, &suffixes)
            .ok_or_else(|| ProcError::NotFound(name.to_owned()));
    }
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            if let Some(found) = first_match(&dir.join(name), &suffixes) {
                return Ok(found);
            }
        }
    }
    Err(ProcError::NotFound(name.to_owned()))
}

fn first_match(base: &Path, suffixes: &[String]) -> Option<PathBuf> {
    suffixes.iter().find_map(|suffix| {
        let mut full = base.as_os_str().to_os_string();
        full.push(suffix);
        let full = PathBuf::from(full);
        is_executable_file(&full).then_some(full)
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wg-proc-resolve-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_tool(dir: &Path, file: &str) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// Paths equal ignoring case (Windows file systems are case-insensitive and `PATHEXT` is
    /// upper-case).
    fn same_file(a: &Path, b: &Path) -> bool {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }

    #[test]
    fn rejects_invalid_names() {
        for bad in ["", "   ", "a\0b"] {
            assert!(matches!(
                resolve_executable_in(bad, None, None),
                Err(ProcError::InvalidExecutable)
            ));
        }
    }

    #[test]
    fn missing_is_not_found() {
        let dir = scratch("missing");
        let err = resolve_executable_in("nope-tool", Some(dir.as_os_str()), None).unwrap_err();
        assert!(matches!(err, ProcError::NotFound(_)));
        let err = resolve_executable_in("sub/nope-tool", Some(dir.as_os_str()), None).unwrap_err();
        assert!(matches!(err, ProcError::NotFound(_)));
    }

    #[test]
    fn finds_on_path_and_by_explicit_path() {
        let dir = scratch("found");
        let file = if cfg!(windows) {
            "wgtool.cmd"
        } else {
            "wgtool"
        };
        let tool = make_tool(&dir, file);
        let found = resolve_executable_in(
            "wgtool",
            Some(dir.as_os_str()),
            Some(OsStr::new(".EXE;.CMD")),
        )
        .unwrap();
        assert!(same_file(&found, &tool), "{found:?}");
        let explicit = resolve_executable_in(tool.to_str().unwrap(), None, None).unwrap();
        assert_eq!(explicit, tool);
    }

    #[cfg(windows)]
    #[test]
    fn pathext_order_decides_and_bare_name_needs_extension() {
        let dir = scratch("pathext");
        make_tool(&dir, "both.cmd");
        let exe = make_tool(&dir, "both.exe");
        make_tool(&dir, "plain");
        let found =
            resolve_executable_in("both", Some(dir.as_os_str()), Some(OsStr::new(".EXE;.CMD")))
                .unwrap();
        assert!(same_file(&found, &exe), "{found:?}");
        assert!(resolve_executable_in("plain", Some(dir.as_os_str()), None).is_err());
    }
}
