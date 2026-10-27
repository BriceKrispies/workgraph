//! Path boundaries: lexical containment, the allowed-roots check and the relative-path guard.
//!
//! The rules of `core/fsutil.py` (`is_within`, `require_within`, `safe_relative_path`) and of the
//! allowed-roots check in `core/services/repositories.py`, as pure string functions. Python's
//! `Path.resolve()` follows symlinks and touches the filesystem, which a pure crate cannot do: the
//! shell canonicalises paths (and strips `\\?\` prefixes, plan R8) before it calls in, and this
//! module compares them lexically. `\` is a separator, as on Windows. A path with a drive letter
//! is compared case-insensitively, as Windows paths are.

use std::iter::once;

use serde_json::Value;
use wg_model::{JsonObject, py_repr_str};
use wg_types::WgError;

/// A path split into its anchor (`""` for relative, `"/"`, or `"C:"` / `"C:/"`) and its
/// components, with `.` dropped and `..` resolved lexically.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lexical {
    root: String,
    parts: Vec<String>,
}

impl Lexical {
    fn is_windows(&self) -> bool {
        self.root
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && self.root.contains(':')
    }

    fn render(&self) -> String {
        format!("{}{}", self.root, self.parts.join("/"))
    }
}

fn split_root(path: &str) -> (String, &str) {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if path[2..].starts_with('/') {
            (format!("{}/", &path[..2]), &path[3..])
        } else {
            (path[..2].to_owned(), &path[2..])
        }
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/".to_owned(), rest)
    } else {
        (String::new(), path)
    }
}

fn lexical(path: &str) -> Lexical {
    let slashed = path.replace('\\', "/");
    let (root, rest) = split_root(&slashed);
    let parts = rest
        .split('/')
        .fold(Vec::new(), |parts: Vec<String>, part| match part {
            "" | "." => parts,
            ".." => match parts.last() {
                Some(last) if last != ".." => parts[..parts.len() - 1].to_vec(),
                // Above an anchored root there is nothing further up; the root stays.
                _ if !root.is_empty() => parts,
                _ => parts.into_iter().chain(once("..".to_owned())).collect(),
            },
            other => parts.into_iter().chain(once(other.to_owned())).collect(),
        });
    Lexical { root, parts }
}

/// Whether `child` is `parent` or lies beneath it (Python's `child.relative_to(parent)` not
/// raising). Both are compared after lexical normalisation.
pub fn is_within(child: &str, parent: &str) -> bool {
    let child = lexical(child);
    let parent = lexical(parent);
    let same = |a: &str, b: &str| {
        if child.is_windows() || parent.is_windows() {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    same(&child.root, &parent.root)
        && child.parts.len() >= parent.parts.len()
        && parent
            .parts
            .iter()
            .zip(&child.parts)
            .all(|(wanted, have)| same(have, wanted))
}

/// `is_within` as an error: the normalised child path when it lies beneath `parent`, else a
/// validation failure whose details carry both paths (Python's `require_within`).
pub fn require_within(child: &str, parent: &str) -> Result<String, WgError> {
    let normalised = lexical(child).render();
    if is_within(child, parent) {
        Ok(normalised)
    } else {
        Err(WgError::validation(format!(
            "path {normalised} escapes the permitted boundary {}",
            lexical(parent).render()
        ))
        .detail("path", Value::String(normalised))
        .detail("boundary", Value::String(lexical(parent).render())))
    }
}

/// A repository-relative path received from untrusted input: non-empty, no NUL, not anchored,
/// no drive, no `..` component. Returns its normalised form (Python's `safe_relative_path`).
pub fn safe_relative_path(value: &str) -> Result<String, WgError> {
    if value.is_empty() || value.contains('\0') {
        return Err(WgError::validation("empty or NUL-containing path"));
    }
    let candidate = lexical(value);
    let traverses = value.replace('\\', "/").split('/').any(|part| part == "..");
    if !candidate.root.is_empty() || traverses {
        return Err(WgError::validation(format!(
            "path {} must be relative and must not traverse upwards",
            py_repr_str(value)
        )));
    }
    Ok(candidate.render())
}

/// The workspace's `allowed_roots` setting as a list of path strings; empty when unset.
pub fn allowed_roots(settings: &JsonObject) -> Vec<String> {
    settings
        .get("allowed_roots")
        .and_then(Value::as_array)
        .map(|roots| {
            roots
                .iter()
                .filter_map(|root| root.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The allowed-roots rule of repository registration: with no roots configured every path is
/// permitted; otherwise the path must lie beneath one of them. The error carries the path and the
/// roots, as Python's `ValidationFailed` does.
pub fn check_allowed_roots(path: &str, settings: &JsonObject) -> Result<(), WgError> {
    let roots = allowed_roots(settings);
    if roots.is_empty() || roots.iter().any(|root| is_within(path, root)) {
        return Ok(());
    }
    Err(
        WgError::validation(format!("{path} is outside the workspace's allowed roots"))
            .detail("path", Value::String(path.to_owned()))
            .detail(
                "allowed_roots",
                Value::Array(roots.into_iter().map(Value::String).collect()),
            ),
    )
}
