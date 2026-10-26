//! `git status --porcelain=v2 --branch -z` records.

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
    let mut status = WorkingTreeStatus::default();
    let entries: Vec<&str> = output.split('\0').collect();
    let mut i = 0;
    while i < entries.len() {
        let entry = entries[i];
        i += 1;
        if entry.is_empty() {
            continue;
        }
        if let Some(oid) = entry.strip_prefix("# branch.oid ") {
            status.head = if oid == "(initial)" {
                None
            } else {
                Some(oid.to_owned())
            };
        } else if let Some(head) = entry.strip_prefix("# branch.head ") {
            status.branch = if head == "(detached)" {
                None
            } else {
                Some(head.to_owned())
            };
        } else if let Some(ab) = entry.strip_prefix("# branch.ab ") {
            let mut parts = ab.split(' ');
            status.ahead = parts
                .next()
                .and_then(|value| value.trim_start_matches('+').parse().ok());
            status.behind = parts
                .next()
                .and_then(|value| value.trim_start_matches('-').parse().ok());
        } else if entry.starts_with("1 ") || entry.starts_with("u ") || entry.starts_with("2 ") {
            let renamed = entry.starts_with("2 ");
            let fields: Vec<&str> = entry.splitn(if renamed { 10 } else { 9 }, ' ').collect();
            let (Some(xy), Some(path)) = (fields.get(1), fields.last()) else {
                continue;
            };
            if renamed {
                i += 1; // the original path follows as its own NUL-terminated field
            }
            let mut codes = xy.chars();
            let staged = codes.next().unwrap_or('.');
            let unstaged = codes.next().unwrap_or('.');
            if staged != '.' {
                status.staged.push((*path).to_owned());
            }
            if unstaged != '.' {
                status.unstaged.push((*path).to_owned());
            }
        } else if let Some(path) = entry.strip_prefix("? ") {
            status.untracked.push(path.to_owned());
        }
    }
    status
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
