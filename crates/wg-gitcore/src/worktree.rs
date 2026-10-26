//! `git worktree list --porcelain` records.

use std::collections::BTreeMap;

/// One record of `git worktree list --porcelain`. The first record is the main working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Parses porcelain output. `nul` selects `-z` framing (NUL terminators and NUL-separated
/// records); otherwise lines end with newlines. An empty field closes the current record. A
/// `worktree` key always starts a new record.
pub fn parse_worktree_porcelain(output: &str, nul: bool) -> Vec<WorktreeEntry> {
    let separator = if nul { '\0' } else { '\n' };
    let mut records: Vec<BTreeMap<&str, &str>> = Vec::new();
    let mut current: Option<usize> = None;
    for line in output.split(separator) {
        if line.is_empty() {
            current = None;
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            let mut record = BTreeMap::new();
            record.insert("worktree", value);
            records.push(record);
            current = Some(records.len() - 1);
        } else if let Some(record) = current.and_then(|index| records.get_mut(index)) {
            record.insert(key, value);
        }
    }
    records
        .into_iter()
        .map(|record| WorktreeEntry {
            path: record
                .get("worktree")
                .copied()
                .unwrap_or_default()
                .to_owned(),
            head: record
                .get("HEAD")
                .filter(|head| !head.is_empty())
                .map(|head| (*head).to_owned()),
            branch: record.get("branch").map(|branch| {
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned()
            }),
            detached: record.contains_key("detached"),
            bare: record.contains_key("bare"),
            locked: record.contains_key("locked"),
            prunable: record.contains_key("prunable"),
        })
        .collect()
}
