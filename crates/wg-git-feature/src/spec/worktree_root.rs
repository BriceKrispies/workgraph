//! Pure: the names and the bounds of attempt directories under a configured
//! `execution.worktree_root`.

use std::path::Path;

use wg_registry::boundary::is_within;
use wg_types::Id;

/// `<task-key>-<n>`, lowercased, with anything but letters, digits and `-` replaced by `-`.
#[must_use]
pub fn attempt_dir_name(task_key: &str, number: i64) -> String {
    let slug: String = task_key
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{slug}-{number}")
}

/// The name used when [`attempt_dir_name`] is already taken: the attempt id's first 8 characters
/// appended.
#[must_use]
pub fn taken_attempt_dir_name(name: &str, attempt_id: Id) -> String {
    let id = attempt_id.to_string();
    format!("{name}-{}", &id[..id.len().min(8)])
}

/// Whether `path` is a worktree in an attempt directory directly below one of `roots`.
#[must_use]
pub fn in_attempt_dir(path: &Path, roots: &[impl AsRef<Path>]) -> bool {
    let Some(grandparent) = path.parent().and_then(Path::parent) else {
        return false;
    };
    let grandparent = grandparent.display().to_string();
    roots.iter().any(|root| {
        let root = root.as_ref().display().to_string();
        is_within(&grandparent, &root) && is_within(&root, &grandparent)
    })
}
