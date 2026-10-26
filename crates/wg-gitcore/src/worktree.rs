//! `git worktree list --porcelain` records.

use rpds::{RedBlackTreeMap, Vector};

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
    // `done` holds the finished records in order; `current` is the record lines are added to.
    let (done, current) = output.split(separator).fold(
        (Vector::<Record>::new(), None::<Record>),
        |(done, current), line| {
            if line.is_empty() {
                return (close(done, current), None);
            }
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            if key == "worktree" {
                let record = RedBlackTreeMap::new().insert("worktree", value);
                (close(done, current), Some(record))
            } else {
                let current = current.map(|record| record.insert(key, value));
                (done, current)
            }
        },
    );
    close(done, current)
        .iter()
        .map(|record| WorktreeEntry {
            path: record
                .get(&"worktree")
                .copied()
                .unwrap_or_default()
                .to_owned(),
            head: record
                .get(&"HEAD")
                .filter(|head| !head.is_empty())
                .map(|head| (*head).to_owned()),
            branch: record.get(&"branch").map(|branch| {
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned()
            }),
            detached: record.contains_key(&"detached"),
            bare: record.contains_key(&"bare"),
            locked: record.contains_key(&"locked"),
            prunable: record.contains_key(&"prunable"),
        })
        .collect()
}

/// The keys of one record, `worktree` first.
type Record<'a> = RedBlackTreeMap<&'a str, &'a str>;

/// Moves the record being read, if any, onto the finished list.
fn close<'a>(done: Vector<Record<'a>>, current: Option<Record<'a>>) -> Vector<Record<'a>> {
    match current {
        Some(record) => done.push_back(record),
        None => done,
    }
}
