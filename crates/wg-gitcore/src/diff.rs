//! Change surfaces of commits: `git diff-tree --stdin -z -r -M -C --root --raw --numstat`, the
//! `git diff --numstat -z -M` of a range, and the `git patch-id --stable` table.

use std::collections::BTreeMap;

use rpds::{RedBlackTreeMap, Vector};

/// One changed file. `status` is the one-letter git status (`A`, `M`, `D`, `R`, `C`, `T`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub similarity: Option<u64>,
    pub insertions: u64,
    pub deletions: u64,
    pub binary: bool,
}

impl FileChange {
    fn new(path: String, status: &str) -> Self {
        Self {
            path,
            old_path: None,
            status: status.to_owned(),
            similarity: None,
            insertions: 0,
            deletions: 0,
            binary: false,
        }
    }
}

/// The files one commit changed, in the order git printed them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiff {
    pub sha: String,
    pub files: Vec<FileChange>,
}

/// A commit header is a full sha: 40 lower-case hex digits, or 64 for SHA-256 repositories.
fn is_commit_sha(token: &str) -> bool {
    matches!(token.len(), 40 | 64)
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A numstat count: decimal digits, or `-` for a binary file.
fn is_count(field: &str) -> bool {
    field == "-" || (!field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()))
}

/// `<added>\t<deleted>\t<path>` with both counts valid. The path may be empty (a rename or copy,
/// whose old and new paths follow as tokens of their own) and may contain tabs or newlines.
fn numstat_fields(token: &str) -> Option<(&str, &str, &str)> {
    let (added, rest) = token.split_once('\t')?;
    let (deleted, path) = rest.split_once('\t')?;
    (is_count(added) && is_count(deleted)).then_some((added, deleted, path))
}

fn count(field: &str) -> u64 {
    field.parse().unwrap_or(0)
}

/// The commit whose records are being read: its sha, its files so far and the position of each
/// path in `files`.
struct Building {
    sha: String,
    files: Vector<FileChange>,
    by_path: RedBlackTreeMap<String, usize>,
}

/// The state of [`parse_diff_tree`] between tokens: the first token not yet consumed by a record,
/// the commit being read, and the commits finished so far keyed by sha.
struct TreeScan {
    next: usize,
    current: Option<Building>,
    result: RedBlackTreeMap<String, CommitDiff>,
}

/// Parses `git diff-tree --stdin -z -r -M -C --root --raw --numstat`. Keyed by commit sha.
///
/// Raw records (`:mode mode sha sha status`, then one path, or two for a rename or copy) and
/// numstat records are merged per commit; a numstat record for a path the raw part did not name
/// adds the file with status `M`. Records before the first commit header are ignored.
pub fn parse_diff_tree(output: &str) -> BTreeMap<String, CommitDiff> {
    let tokens: Vec<&str> = output.split('\0').collect();
    let start = TreeScan {
        next: 0,
        current: None,
        result: RedBlackTreeMap::new(),
    };
    let scan = (0..tokens.len()).fold(start, |scan, index| scan_tree_token(&tokens, index, scan));
    let result = match scan.current {
        Some(building) => insert_finished(scan.result, building),
        None => scan.result,
    };
    result
        .iter()
        .map(|(sha, diff)| (sha.clone(), diff.clone()))
        .collect()
}

/// Reads the token at `index` unless an earlier record already consumed it.
fn scan_tree_token(tokens: &[&str], index: usize, scan: TreeScan) -> TreeScan {
    if index < scan.next {
        return scan;
    }
    let token = tokens[index];
    let next = index + 1;
    let token_at = |offset: usize| tokens.get(offset).copied().unwrap_or("");
    let TreeScan {
        current, result, ..
    } = scan;
    let Some(building) = current else {
        // Before the first commit header only a header means anything.
        let current = is_commit_sha(token).then(|| new_building(token));
        return TreeScan {
            next,
            current,
            result,
        };
    };
    if token.is_empty() {
        TreeScan {
            next,
            current: Some(building),
            result,
        }
    } else if let Some(raw) = token.strip_prefix(':') {
        let status_field = raw.split(' ').next_back().unwrap_or("M");
        let status = status_field.chars().next().unwrap_or('M').to_string();
        let rest = status_field.get(status.len()..).unwrap_or("");
        let similarity =
            (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then(|| count(rest));
        let (change, consumed) = if status == "R" || status == "C" {
            let change = FileChange {
                old_path: Some(token_at(index + 1).to_owned()),
                similarity,
                ..FileChange::new(token_at(index + 2).to_owned(), &status)
            };
            (change, index + 3)
        } else {
            let change = FileChange::new(token_at(index + 1).to_owned(), &status);
            (change, index + 2)
        };
        let position = building.files.len();
        let by_path = building.by_path.insert(change.path.clone(), position);
        TreeScan {
            next: consumed,
            current: Some(Building {
                sha: building.sha,
                files: building.files.push_back(change),
                by_path,
            }),
            result,
        }
    } else if let Some((added, deleted, path)) = numstat_fields(token) {
        let (path, consumed) = if path.is_empty() {
            // Rename or copy: the old path, then the new one, follow.
            (token_at(index + 2).to_owned(), index + 3)
        } else {
            (path.to_owned(), next)
        };
        let (building, position) = match building.by_path.get(&path) {
            Some(&position) => (building, position),
            None => {
                let position = building.files.len();
                let grown = Building {
                    sha: building.sha,
                    files: building.files.push_back(FileChange::new(path.clone(), "M")),
                    by_path: building.by_path.insert(path, position),
                };
                (grown, position)
            }
        };
        let binary = added == "-" || deleted == "-";
        let files = update_file(building.files, position, |file| {
            if binary {
                FileChange {
                    binary: true,
                    ..file.clone()
                }
            } else {
                FileChange {
                    insertions: count(added),
                    deletions: count(deleted),
                    ..file.clone()
                }
            }
        });
        TreeScan {
            next: consumed,
            current: Some(Building {
                sha: building.sha,
                files,
                by_path: building.by_path,
            }),
            result,
        }
    } else if is_commit_sha(token) {
        TreeScan {
            next,
            current: Some(new_building(token)),
            result: insert_finished(result, building),
        }
    } else {
        TreeScan {
            next,
            current: Some(building),
            result,
        }
    }
}

fn new_building(sha: &str) -> Building {
    Building {
        sha: sha.to_owned(),
        files: Vector::new(),
        by_path: RedBlackTreeMap::new(),
    }
}

/// Records a finished commit in `result`, replacing any earlier record with the same sha.
fn insert_finished(
    result: RedBlackTreeMap<String, CommitDiff>,
    building: Building,
) -> RedBlackTreeMap<String, CommitDiff> {
    let files = building.files.iter().cloned().collect();
    result.insert(
        building.sha.clone(),
        CommitDiff {
            sha: building.sha,
            files,
        },
    )
}

/// Replaces the file at `position` with `edit` applied to it. A position out of range leaves the
/// list as it is.
fn update_file(
    files: Vector<FileChange>,
    position: usize,
    edit: impl FnOnce(&FileChange) -> FileChange,
) -> Vector<FileChange> {
    match files.get(position).map(edit) {
        Some(change) => files.set(position, change).unwrap_or(files),
        None => files,
    }
}

/// Parses `git diff --numstat -z -M` (the change surface of a range). Rename status is `R`, every
/// other entry `M`; similarity is not reported by numstat and stays `None`.
pub fn parse_numstat_z(output: &str) -> Vec<FileChange> {
    let tokens: Vec<&str> = output.split('\0').collect();
    let token_at = |offset: usize| tokens.get(offset).copied().unwrap_or("");
    // The state is the first token not yet consumed and the files read so far.
    let (_, files) = (0..tokens.len()).fold(
        (0_usize, Vector::<FileChange>::new()),
        |(next, files), index| {
            if index < next {
                return (next, files);
            }
            let Some((added, deleted, path)) = numstat_fields(tokens[index]) else {
                return (index + 1, files);
            };
            let binary = added == "-" || deleted == "-";
            let (path, old_path, status, consumed) = if path.is_empty() {
                // Rename or copy: the old path, then the new one, follow.
                (
                    token_at(index + 2).to_owned(),
                    Some(token_at(index + 1).to_owned()),
                    "R",
                    index + 3,
                )
            } else {
                (path.to_owned(), None, "M", index + 1)
            };
            let file = FileChange {
                path,
                old_path,
                status: status.to_owned(),
                similarity: None,
                insertions: if binary { 0 } else { count(added) },
                deletions: if binary { 0 } else { count(deleted) },
                binary,
            };
            (consumed, files.push_back(file))
        },
    );
    files.iter().cloned().collect()
}

/// Parses `git patch-id --stable` output (`<patch-id> <commit>` per line) into `{commit: patch_id}`.
/// Lines without exactly two fields are skipped.
pub fn parse_patch_ids(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                [patch_id, commit] => Some(((*commit).to_owned(), (*patch_id).to_owned())),
                _ => None,
            }
        })
        .collect()
}
