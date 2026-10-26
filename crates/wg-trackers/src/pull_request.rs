//! Pull request references: URL parsing and the URLs named in free text.

use std::collections::BTreeSet;

use rpds::RedBlackTreeMap;

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

/// Every `http://` or `https://` URL in free text. A URL runs to the first whitespace, angle
/// bracket, quote, square bracket or parenthesis; trailing punctuation is the caller's concern.
#[must_use]
pub fn urls_in(text: &str) -> Vec<&str> {
    std::iter::successors(url_span(text, 0), |&(_, end)| url_span(text, end))
        .map(|(begin, end)| &text[begin..end])
        .collect()
}

/// The first URL at or after `from`, as its byte span. A scheme with an empty body is skipped.
fn url_span(text: &str, from: usize) -> Option<(usize, usize)> {
    std::iter::successors(next_scheme(text, from), |&(begin, _)| {
        next_scheme(text, begin + 1)
    })
    .filter_map(|(begin, scheme_len)| {
        let body = begin + scheme_len;
        let end = text[body..]
            .find(|c: char| c.is_whitespace() || "<>\"'[]()".contains(c))
            .map_or(text.len(), |offset| body + offset);
        (end > body).then_some((begin, end))
    })
    .next()
}

/// The next `http://` or `https://` at or after `from`: its start and the length of its scheme.
fn next_scheme(text: &str, from: usize) -> Option<(usize, usize)> {
    text.get(from..)?
        .match_indices("http")
        .map(|(offset, _)| from + offset)
        .find_map(|begin| {
            let tail = &text[begin + 4..];
            if tail.starts_with("://") {
                Some((begin, 7))
            } else if tail.starts_with("s://") {
                Some((begin, 8))
            } else {
                None
            }
        })
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
