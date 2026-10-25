//! The pure rules of executable resolution.

use crate::ProcError;

/// Reject an empty (or blank) executable name or one with a NUL byte.
///
/// # Errors
/// [`ProcError::InvalidExecutable`].
pub fn validate_executable_name(name: &str) -> Result<(), ProcError> {
    if name.contains('\0') || name.trim().is_empty() {
        Err(ProcError::InvalidExecutable)
    } else {
        Ok(())
    }
}

/// The suffixes to try after a candidate path, in order; an empty suffix means "as written".
///
/// Outside Windows that is just the empty suffix. On Windows it is each `PATHEXT` entry
/// (default `.COM;.EXE;.BAT;.CMD`), preceded by the empty suffix only when `extension` (the
/// candidate's own extension, without the dot) is already one of them. Like `shutil.which`, a
/// bare `git` therefore never matches an extensionless file.
pub fn executable_suffixes(
    extension: Option<&str>,
    pathext: Option<&str>,
    windows: bool,
) -> Vec<String> {
    if !windows {
        return vec![String::new()];
    }
    let configured = pathext.unwrap_or(".COM;.EXE;.BAT;.CMD");
    let list: Vec<String> = configured
        .split(';')
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect();
    let has_known = extension.is_some_and(|ext| {
        list.iter()
            .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(ext))
    });
    if has_known {
        let mut suffixes = vec![String::new()];
        suffixes.extend(list);
        suffixes
    } else {
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate_executable_name("git").is_ok());
        for bad in ["", "  ", "a\0b"] {
            assert!(validate_executable_name(bad).is_err());
        }
    }

    #[test]
    fn suffixes() {
        assert_eq!(executable_suffixes(None, None, false), [""]);
        assert_eq!(
            executable_suffixes(None, None, true),
            [".COM", ".EXE", ".BAT", ".CMD"]
        );
        assert_eq!(
            executable_suffixes(Some("exe"), Some(".EXE;.CMD"), true),
            ["", ".EXE", ".CMD"]
        );
        assert_eq!(
            executable_suffixes(Some("txt"), Some(".EXE;;.CMD"), true),
            [".EXE", ".CMD"]
        );
    }
}
