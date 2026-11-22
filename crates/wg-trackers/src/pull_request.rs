//! Pull request references: URL parsing and the URLs named in free text.

use std::collections::BTreeSet;

use rpds::{RedBlackTreeMap, Vector};

/// Source of a pull request found in a configured custom field.
pub const SOURCE_CUSTOM_FIELD: &str = "custom_field";
/// Source of a pull request found in the development panel.
pub const SOURCE_DEV_PANEL: &str = "dev_panel";

const PULL_SUFFIXES: &[&str] = &["files", "commits", "checks"];

/// A pull request named by an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestRef {
    /// `https://host/org/repo/pull/42`, with the repository name in the case it was given.
    pub url: String,
    /// `github.com/org/repo/pull/42`: lowercase, no scheme, no trailing slash or fragment.
    pub normalized_url: String,
    /// `github` for now.
    pub provider: &'static str,
    /// `org/repo`, in the case it was given.
    pub repo_full_name: String,
    /// `github.com/org/repo`, comparable with the normalised remote URL of a repository.
    pub repo_normalized_url: String,
    pub number: u64,
    /// Which extractors found it, sorted: `custom_field` and/or `dev_panel`.
    pub sources: Vec<&'static str>,
    /// The development-panel title, or empty.
    pub title: String,
}

/// Parses `https://<host>/<org>/<repo>/pull/<n>` on any host, or `None` for anything else.
///
/// Accepts a trailing `/files`, `/commits` or `/checks`, a query, a fragment and `www.`. The
/// normalised forms are lowercase; `repo_full_name` keeps the case it was given in.
#[must_use]
pub fn parse_pull_request_url(url: &str) -> Option<PullRequestRef> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let path_and_more = &rest[authority_end..];
    let path = path_and_more.split(['?', '#']).next().unwrap_or_default();

    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_port)| host_port);
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _)| host)
        .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }

    let all_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let segments: &[&str] = if all_segments.len() == 5 && PULL_SUFFIXES.contains(&all_segments[4]) {
        &all_segments[..4]
    } else {
        &all_segments
    };
    let (org, repo, number_text) = match segments {
        [org, repo, "pull", number] => (*org, *repo, *number),
        _ => return None,
    };
    if number_text.is_empty() || !number_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u64 = number_text.parse().ok()?;
    if number < 1 {
        return None;
    }

    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let repo_full_name = format!("{org}/{repo}");
    let repo_remote = format!("https://{host}/{repo_full_name}");
    Some(PullRequestRef {
        url: format!("https://{host}/{repo_full_name}/pull/{number}"),
        normalized_url: format!("{host}/{repo_full_name}/pull/{number}").to_lowercase(),
        provider: "github",
        repo_normalized_url: wg_registry::remote::normalize_remote_url(&repo_remote).to_lowercase(),
        repo_full_name,
        number,
        sources: Vec::new(),
        title: String::new(),
    })
}

/// The pull requests one field's text names, in order.
///
/// Full URLs need no scheme. `pull/42` shorthand and numbers listed straight after a reference
/// ("pull/112, 114 and 120") take the repository of the last full URL, else `default_repo`. A
/// text of bare numbers ("1749, #1809") names pull requests in `default_repo`. Without a
/// repository, shorthand is skipped rather than guessed.
#[must_use]
pub fn field_pull_requests(text: &str, default_repo: Option<&str>) -> Vec<PullRequestRef> {
    let default = default_repo.map(repo_base);
    match default.as_deref() {
        Some(base) if only_numbers(text) => bare_tokens(text)
            .filter_map(|token| bare_number(token).flatten())
            .filter_map(|number| reference(base, number))
            .collect(),
        _ => {
            let matches = std::iter::successors(next_reference(text, 0), |(_, end)| {
                next_reference(text, *end)
            });
            let (_, found) = matches.fold(
                (default, Vector::new_sync()),
                |(repo, found), (matched, end)| {
                    let (repo, head) = match matched {
                        Matched::Full { repo: full, number } => (Some(full), Some(number)),
                        Matched::Short(number) => {
                            let head = repo.as_ref().map(|_| number);
                            (repo, head)
                        }
                    };
                    let numbers = head.into_iter().chain(match repo {
                        Some(_) => continuations(text, end),
                        None => Vec::new(),
                    });
                    let found = numbers
                        .filter_map(|number| reference(repo.as_deref()?, number))
                        .fold(found, |found, reference| found.push_back(reference));
                    (repo, found)
                },
            );
            found.iter().cloned().collect()
        }
    }
}

/// The numbers listed straight after a reference that ends at `end`.
fn continuations(text: &str, end: usize) -> Vec<&str> {
    std::iter::successors(continuation(text, end), |(_, next)| {
        continuation(text, *next)
    })
    .map(|(number, _)| number)
    .collect()
}
enum Matched<'a> {
    /// `host/org/repo/pull/n`, with `repo` as `https://host/org/repo`.
    Full { repo: String, number: &'a str },
    /// `pull/n`.
    Short(&'a str),
}

fn reference(repo: &str, number: &str) -> Option<PullRequestRef> {
    parse_pull_request_url(&format!("{repo}/pull/{number}"))
}

fn repo_base(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_string()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn char_before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

/// The end of a run of chars satisfying `keep`, from `at`.
fn run_end(text: &str, at: usize, keep: impl Fn(char) -> bool) -> usize {
    text[at..]
        .char_indices()
        .find(|&(_, c)| !keep(c))
        .map_or(text.len(), |(offset, _)| at + offset)
}

fn starts_with_ci(text: &str, at: usize, prefix: &str) -> bool {
    text.get(at..at + prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// The first reference at or after `from`, and where it ends.
fn next_reference(text: &str, from: usize) -> Option<(Matched<'_>, usize)> {
    text[from..]
        .char_indices()
        .map(|(offset, _)| from + offset)
        .find_map(|at| full_reference(text, at).or_else(|| short_reference(text, at)))
}

/// `(https?://)?host/org/repo/pull/n` starting at `at`, not inside a longer word or path.
fn full_reference(text: &str, at: usize) -> Option<(Matched<'_>, usize)> {
    if char_before(text, at).is_some_and(|c| is_word(c) || ".@/-".contains(c)) {
        return None;
    }
    let cursor = if starts_with_ci(text, at, "https://") {
        at + 8
    } else if starts_with_ci(text, at, "http://") {
        at + 7
    } else {
        at
    };
    let host_end = run_end(text, cursor, |c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '.'
    });
    let host = &text[cursor..host_end];
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        return None;
    }
    let segment = |c: char| is_word(c) || c == '.' || c == '-';
    let org_start = slash_after(text, host_end)?;
    let org_end = run_end(text, org_start, segment);
    let repo_start = slash_after(text, org_end).filter(|_| org_end > org_start)?;
    let repo_end = run_end(text, repo_start, segment);
    let pull_start = slash_after(text, repo_end).filter(|_| repo_end > repo_start)?;
    let (number, end) = pull_number(text, pull_start)?;
    let repo = format!(
        "https://{host}/{}/{}",
        &text[org_start..org_end],
        &text[repo_start..repo_end]
    );
    Some((Matched::Full { repo, number }, end))
}

/// `/?pull/n` starting at `at`, not inside a longer word or path.
fn short_reference(text: &str, at: usize) -> Option<(Matched<'_>, usize)> {
    if char_before(text, at).is_some_and(|c| is_word(c) || "./-".contains(c)) {
        return None;
    }
    let start = if text[at..].starts_with('/') {
        at + 1
    } else {
        at
    };
    let (number, end) = pull_number(text, start)?;
    Some((Matched::Short(number), end))
}

/// The position after a `/` at `at`.
fn slash_after(text: &str, at: usize) -> Option<usize> {
    text[at..].starts_with('/').then_some(at + 1)
}

/// `pull/<digits>` at `at`: the digits and where they end.
fn pull_number(text: &str, at: usize) -> Option<(&str, usize)> {
    if !starts_with_ci(text, at, "pull/") {
        return None;
    }
    let start = at + 5;
    let end = run_end(text, start, |c| c.is_ascii_digit());
    (end > start).then(|| (&text[start..end], end))
}

/// `, 114`, `& 114` or ` and #114` at `at`: a number listed after a reference.
fn continuation(text: &str, at: usize) -> Option<(&str, usize)> {
    let separator = run_end(text, at, char::is_whitespace);
    let after_separator = if text[separator..].starts_with([',', '&']) {
        separator + 1
    } else if starts_with_ci(text, separator, "and")
        && !char_before(text, separator).is_some_and(is_word)
        && !text[separator + 3..].chars().next().is_some_and(is_word)
    {
        separator + 3
    } else {
        return None;
    };
    let spaced = run_end(text, after_separator, char::is_whitespace);
    let start = if text[spaced..].starts_with('#') {
        spaced + 1
    } else {
        spaced
    };
    let end = run_end(text, start, |c| c.is_ascii_digit());
    let after_ok = !text[end..]
        .chars()
        .next()
        .is_some_and(|c| is_word(c) || "-./".contains(c));
    ((1..=6).contains(&(end - start)) && after_ok).then(|| (&text[start..end], end))
}
fn bare_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || ",;/&+".contains(c))
        .filter(|token| !token.is_empty())
}

/// A token of a bare-number field: `Some(Some(n))` for `42`, `#42`, `pr42` or `and #42`;
/// `Some(None)` for a lone `pr`, `pull` or `and`; `None` for anything else.
fn bare_number(token: &str) -> Option<Option<&str>> {
    let lower = token.to_ascii_lowercase();
    if matches!(lower.as_str(), "pr" | "pull" | "and") {
        return Some(None);
    }
    let rest = ["pull", "pr", "and"]
        .iter()
        .find_map(|prefix| starts_with_ci(token, 0, prefix).then(|| &token[prefix.len()..]))
        .unwrap_or(token);
    let digits = rest.strip_prefix('#').unwrap_or(rest);
    let numeric = digits.bytes().all(|b| b.is_ascii_digit());
    (numeric && (2..=6).contains(&digits.len())).then_some(Some(digits))
}

fn only_numbers(text: &str) -> bool {
    let tokens: Vec<&str> = bare_tokens(text).collect();
    !tokens.is_empty() && tokens.iter().all(|token| bare_number(token).is_some())
}

/// Merges references to the same pull request: one entry per normalised URL, sources united,
/// the first non-empty title kept, sorted by normalised URL.
pub(crate) fn merge(found: Vec<PullRequestRef>) -> Vec<PullRequestRef> {
    found
        .into_iter()
        .fold(RedBlackTreeMap::new(), |merged, reference| {
            match merged.get(&reference.normalized_url) {
                None => merged.insert(reference.normalized_url.clone(), reference),
                Some(current) => {
                    let sources: BTreeSet<&'static str> = current
                        .sources
                        .iter()
                        .chain(reference.sources.iter())
                        .copied()
                        .collect();
                    let title = if current.title.is_empty() {
                        reference.title.clone()
                    } else {
                        current.title.clone()
                    };
                    let updated = PullRequestRef {
                        sources: sources.into_iter().collect(),
                        title,
                        ..current.clone()
                    };
                    merged.insert(reference.normalized_url.clone(), updated)
                }
            }
        })
        .values()
        .cloned()
        .collect()
}
