//! `git status --porcelain=v2 --branch -z` records.

use rpds::Vector;

/// The working tree as `git status --porcelain=v2 --branch -z` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingTreeStatus {
    /// The branch, `None` when detached.
    pub branch: Option<String>,
    /// The commit at HEAD, `None` before the first commit.
    pub head: Option<String>,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
}

/// Parses `status --porcelain=v2 --branch -z` output. Entries are NUL-terminated; a renamed entry
/// is followed by its original path as a separate field. An unmerged (`u`) entry counts as both
/// staged and unstaged, as the Python integration reads it.
pub fn parse_status_porcelain_v2(output: &str) -> WorkingTreeStatus {
    let entries: Vec<&str> = output.split('\0').collect();
    // A renamed entry is followed by its original path as a field of its own. That field is
    // not an entry, so it is dropped here before the entries are read.
    let (kept, _) = entries.iter().fold(
        (Vector::<&str>::new(), false),
        |(kept, is_origin_path), entry| {
            if is_origin_path {
                (kept, false)
            } else {
                (kept.push_back(*entry), entry.starts_with("2 "))
            }
        },
    );
    let kept: Vec<&str> = kept.iter().copied().collect();

    // The last header line of each kind wins, as a later line overwrites an earlier one.
    let head = kept
        .iter()
        .filter_map(|entry| entry.strip_prefix("# branch.oid "))
        .next_back()
        .and_then(|oid| (oid != "(initial)").then(|| oid.to_owned()));
    let branch = kept
        .iter()
        .filter_map(|entry| entry.strip_prefix("# branch.head "))
        .next_back()
        .and_then(|head| (head != "(detached)").then(|| head.to_owned()));
    let ab: Vec<&str> = kept
        .iter()
        .filter_map(|entry| entry.strip_prefix("# branch.ab "))
        .next_back()
        .map(|ab| ab.split(' ').collect())
        .unwrap_or_default();
    let ahead = ab
        .first()
        .and_then(|value| value.trim_start_matches('+').parse().ok());
    let behind = ab
        .get(1)
        .and_then(|value| value.trim_start_matches('-').parse().ok());

    // Each change record gives its staged and unstaged codes and its path.
    let changes: Vec<(char, char, &str)> = kept
        .iter()
        .filter(|entry| {
            entry.starts_with("1 ") || entry.starts_with("u ") || entry.starts_with("2 ")
        })
        .filter_map(|entry| {
            let renamed = entry.starts_with("2 ");
            let fields: Vec<&str> = entry.splitn(if renamed { 10 } else { 9 }, ' ').collect();
            let (Some(xy), Some(path)) = (fields.get(1), fields.last()) else {
                return None;
            };
            let staged = xy.chars().next().unwrap_or('.');
            let unstaged = xy.chars().nth(1).unwrap_or('.');
            Some((staged, unstaged, *path))
        })
        .collect();
    let staged = changes
        .iter()
        .filter(|(staged, _, _)| *staged != '.')
        .map(|(_, _, path)| (*path).to_owned())
        .collect();
    let unstaged = changes
        .iter()
        .filter(|(_, unstaged, _)| *unstaged != '.')
        .map(|(_, _, path)| (*path).to_owned())
        .collect();
    let untracked = kept
        .iter()
        .filter_map(|entry| entry.strip_prefix("? "))
        .map(str::to_owned)
        .collect();

    WorkingTreeStatus {
        branch,
        head,
        ahead,
        behind,
        staged,
        unstaged,
        untracked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_header_and_changes() {
        let output = "# branch.oid abc\0# branch.head main\0# branch.ab +2 -1\0\
            1 M. N... 100644 100644 100644 h1 h2 staged.txt\0\
            1 .M N... 100644 100644 100644 h1 h2 unstaged.txt\0\
            2 R. N... 100644 100644 100644 h1 h2 R100 new.txt\0old.txt\0\
            ? untracked.txt\0";
        let status = parse_status_porcelain_v2(output);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head.as_deref(), Some("abc"));
        assert_eq!((status.ahead, status.behind), (Some(2), Some(1)));
        assert_eq!(status.staged, ["staged.txt", "new.txt"]);
        assert_eq!(status.unstaged, ["unstaged.txt"]);
        assert_eq!(status.untracked, ["untracked.txt"]);
    }

    #[test]
    fn initial_and_detached_states() {
        let status =
            parse_status_porcelain_v2("# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(status.head, None);
        assert_eq!(status.branch, None);
        assert_eq!(status.ahead, None);
    }
}
