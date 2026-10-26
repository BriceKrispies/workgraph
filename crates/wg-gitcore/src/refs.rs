//! Refs, remotes, commit lists and version text that git prints, and the checks Workgraph applies
//! before it passes a name to git.

use std::collections::{BTreeMap, BTreeSet};

/// `re.fullmatch`-style check of a commit id: 7 to 64 lowercase hexadecimal digits.
pub fn is_sha(value: &str) -> bool {
    (7..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `value` is a ref name safe to pass to git: 1 to 255 characters of
/// `A-Za-z0-9._/@{}^~+-`, not starting with `-`, with no `..` and no `//`.
pub fn is_valid_ref(value: &str) -> bool {
    let allowed = value.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '.' | '_' | '/' | '@' | '{' | '}' | '^' | '~' | '+' | '-')
    });
    (1..=255).contains(&value.len())
        && allowed
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains("//")
}

/// Whitespace-separated words of git output (Python's `str.split()`).
pub fn split_words(output: &str) -> Vec<String> {
    output.split_whitespace().map(str::to_owned).collect()
}

/// The trimmed output when it is not empty, `None` otherwise.
pub fn non_empty_trimmed(output: &str) -> Option<String> {
    let value = output.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The stdin text for git commands that read one item per line: each line terminated by `\n`.
pub fn stdin_lines(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// `git remote -v` lines as `(name, url)` pairs, in order. Only fetch URLs count and the first
/// URL of a name wins.
pub fn parse_remotes(output: &str) -> Vec<(String, String)> {
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let fetch = parts.len() < 3 || parts[2] == "(fetch)";
        if parts.len() >= 2 && fetch && !remotes.iter().any(|(name, _)| name == parts[0]) {
            remotes.push((parts[0].to_owned(), parts[1].to_owned()));
        }
    }
    remotes
}

/// `for-each-ref` output with the format `%(refname)%00%(objectname)%00%(*objectname)`: each
/// ref to its commit id, with annotated tags peeled. `HEAD` entries are skipped.
pub fn parse_refs(output: &str) -> BTreeMap<String, String> {
    let mut refs = BTreeMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\0').collect();
        if let [name, object, peeled] = parts.as_slice()
            && !name.ends_with("/HEAD")
        {
            let sha = if peeled.is_empty() { *object } else { *peeled };
            refs.insert((*name).to_owned(), sha.to_owned());
        }
    }
    refs
}

/// Commit ids in `output`, one per line: trimmed, non-empty, sorted and unique.
pub fn parse_sha_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The major and minor numbers of `git --version` output, `(0, 0)` when there are none.
pub fn parse_git_version(output: &str) -> (u32, u32) {
    let Ok(pattern) = regex::Regex::new(r"git version (\d+)\.(\d+)") else {
        return (0, 0);
    };
    pattern
        .captures(output)
        .and_then(|found| {
            let major = found.get(1)?.as_str().parse().ok()?;
            let minor = found.get(2)?.as_str().parse().ok()?;
            Some((major, minor))
        })
        .unwrap_or((0, 0))
}

/// The branch a remote's `symbolic-ref refs/remotes/<remote>/HEAD` names, from its trimmed output:
/// `refs/remotes/origin/main` gives `main`. Empty output gives `None`.
pub fn symref_branch(output: &str) -> Option<String> {
    let value = output.trim();
    if value.is_empty() {
        return None;
    }
    value.splitn(4, '/').last().map(str::to_owned)
}

/// The default branch, as `WorkingTreeGit.default_branch` picks it: the first remote's HEAD
/// (`remote_heads` holds each remote's `symbolic-ref` output, empty when it failed), then the
/// configured `init.defaultBranch` or one of `main`, `master`, `trunk`, `develop` that exists,
/// then the current branch, then the first local branch.
pub fn choose_default_branch(
    remote_heads: &[String],
    configured: &str,
    branches: &[String],
    current: Option<&str>,
) -> Option<String> {
    if let Some(branch) = remote_heads.iter().find_map(|head| symref_branch(head)) {
        return Some(branch);
    }
    for candidate in [configured, "main", "master", "trunk", "develop"] {
        if !candidate.is_empty() && branches.iter().any(|branch| branch == candidate) {
            return Some(candidate.to_owned());
        }
    }
    current
        .map(str::to_owned)
        .or_else(|| branches.first().cloned())
}

/// The message of a git command that exited non-zero: the first four arguments, the return code
/// and the first 500 characters of the trimmed stderr.
pub fn failed_message(args: &[String], returncode: i32, stderr: &str) -> String {
    let leading = args
        .iter()
        .take(4)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let detail: String = stderr.trim().chars().take(500).collect();
    format!("git {leading} failed ({returncode}): {detail}")
}

/// A path as Python's `Path.as_posix()` renders it: backslashes become slashes on Windows.
pub fn posix_path(path: &str, windows: bool) -> String {
    if windows {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

/// A Windows verbatim prefix (`\\?\`) removed, as Python's `Path.resolve()` leaves it out.
pub fn strip_verbatim_prefix(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_and_ref_checks() {
        assert!(is_sha("abcdef1"));
        assert!(!is_sha("ABCDEF1"));
        assert!(!is_sha("abc"));
        assert!(is_valid_ref("refs/heads/feature-1"));
        assert!(!is_valid_ref("-x"));
        assert!(!is_valid_ref("a..b"));
        assert!(!is_valid_ref("a//b"));
        assert!(!is_valid_ref("a b"));
    }

    #[test]
    fn remotes_and_refs() {
        let remotes = parse_remotes(
            "origin\thttps://h/x (fetch)\norigin\thttps://h/x (push)\nup\tfile:///u (fetch)\n",
        );
        assert_eq!(
            remotes,
            [
                ("origin".to_owned(), "https://h/x".to_owned()),
                ("up".to_owned(), "file:///u".to_owned())
            ]
        );
        let refs = parse_refs(
            "refs/heads/main\x001\x00\nrefs/tags/v1\x002\x003\nrefs/remotes/o/HEAD\x004\x00\n",
        );
        assert_eq!(refs.get("refs/tags/v1").map(String::as_str), Some("3"));
        assert!(!refs.contains_key("refs/remotes/o/HEAD"));
    }

    #[test]
    fn default_branch_order() {
        let branches = vec!["develop".to_owned(), "trunk".to_owned()];
        assert_eq!(
            choose_default_branch(&["refs/remotes/origin/main".to_owned()], "", &[], None),
            Some("main".to_owned())
        );
        assert_eq!(
            choose_default_branch(&[], "", &branches, Some("feature")),
            Some("trunk".to_owned())
        );
        assert_eq!(
            choose_default_branch(&[], "", &[], Some("feature")),
            Some("feature".to_owned())
        );
    }

    #[test]
    fn versions_and_messages() {
        assert_eq!(parse_git_version("git version 2.45.1.windows.1"), (2, 45));
        assert_eq!(parse_git_version("nothing"), (0, 0));
        let message = failed_message(
            &[
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                "x".to_owned(),
                "y".to_owned(),
                "z".to_owned(),
            ],
            128,
            "  fatal  \n",
        );
        assert_eq!(message, "git rev-parse --verify x y failed (128): fatal");
    }
}
