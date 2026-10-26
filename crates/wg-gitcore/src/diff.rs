//! Change surfaces of commits: `git diff-tree --stdin -z -r -M -C --root --raw --numstat`, the
//! `git diff --numstat -z -M` of a range, and the `git patch-id --stable` table.

use std::collections::{BTreeMap, HashMap};

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

/// Parses `git diff-tree --stdin -z -r -M -C --root --raw --numstat`. Keyed by commit sha.
///
/// Raw records (`:mode mode sha sha status`, then one path, or two for a rename or copy) and
/// numstat records are merged per commit; a numstat record for a path the raw part did not name
/// adds the file with status `M`. Records before the first commit header are ignored.
pub fn parse_diff_tree(output: &str) -> BTreeMap<String, CommitDiff> {
    let tokens: Vec<&str> = output.split('\0').collect();
    let token_at = |index: usize| tokens.get(index).copied().unwrap_or("");
    let mut result: BTreeMap<String, CommitDiff> = BTreeMap::new();
    let mut current: Option<CommitDiff> = None;
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = token_at(i);
        i += 1;
        if token.is_empty() {
            continue;
        }
        if let Some(raw) = token.strip_prefix(':') {
            let Some(commit) = current.as_mut() else {
                continue;
            };
            let status_field = raw.split(' ').next_back().unwrap_or("M");
            let mut chars = status_field.chars();
            let status = chars.next().unwrap_or('M').to_string();
            let rest = chars.as_str();
            let similarity =
                (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then(|| count(rest));
            let change = if status == "R" || status == "C" {
                let old_path = token_at(i).to_owned();
                let new_path = token_at(i + 1).to_owned();
                i += 2;
                FileChange {
                    old_path: Some(old_path),
                    similarity,
                    ..FileChange::new(new_path, &status)
                }
            } else {
                let path = token_at(i).to_owned();
                i += 1;
                FileChange::new(path, &status)
            };
            by_path.insert(change.path.clone(), commit.files.len());
            commit.files.push(change);
            continue;
        }
        if let Some((added, deleted, path)) = numstat_fields(token) {
            let Some(commit) = current.as_mut() else {
                continue;
            };
            let mut path = path.to_owned();
            if path.is_empty() {
                // Rename or copy: the old path, then the new one, follow.
                path = token_at(i + 1).to_owned();
                i += 2;
            }
            let index = match by_path.get(&path) {
                Some(&index) => index,
                None => {
                    commit.files.push(FileChange::new(path.clone(), "M"));
                    by_path.insert(path, commit.files.len() - 1);
                    commit.files.len() - 1
                }
            };
            if let Some(change) = commit.files.get_mut(index) {
                if added == "-" || deleted == "-" {
                    change.binary = true;
                } else {
                    change.insertions = count(added);
                    change.deletions = count(deleted);
                }
            }
            continue;
        }
        if is_commit_sha(token) {
            if let Some(finished) = current.take() {
                result.insert(finished.sha.clone(), finished);
            }
            current = Some(CommitDiff {
                sha: token.to_owned(),
                files: Vec::new(),
            });
            by_path.clear();
        }
    }
    if let Some(finished) = current {
        result.insert(finished.sha.clone(), finished);
    }
    result
}

/// Parses `git diff --numstat -z -M` (the change surface of a range). Rename status is `R`, every
/// other entry `M`; similarity is not reported by numstat and stays `None`.
pub fn parse_numstat_z(output: &str) -> Vec<FileChange> {
    let tokens: Vec<&str> = output.split('\0').collect();
    let token_at = |index: usize| tokens.get(index).copied().unwrap_or("");
    let mut files = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = token_at(i);
        i += 1;
        let Some((added, deleted, path)) = numstat_fields(token) else {
            continue;
        };
        let mut path = path.to_owned();
        let mut old_path = None;
        let mut status = "M";
        if path.is_empty() {
            old_path = Some(token_at(i).to_owned());
            path = token_at(i + 1).to_owned();
            i += 2;
            status = "R";
        }
        let binary = added == "-" || deleted == "-";
        files.push(FileChange {
            path,
            old_path,
            status: status.to_owned(),
            similarity: None,
            insertions: if binary { 0 } else { count(added) },
            deletions: if binary { 0 } else { count(deleted) },
            binary,
        });
    }
    files
}

/// Parses `git patch-id --stable` output (`<patch-id> <commit>` per line) into `{commit: patch_id}`.
/// Lines without exactly two fields are skipped.
pub fn parse_patch_ids(output: &str) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let [patch_id, commit] = parts.as_slice() {
            ids.insert((*commit).to_owned(), (*patch_id).to_owned());
        }
    }
    ids
}
