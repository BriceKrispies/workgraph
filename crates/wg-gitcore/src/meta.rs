//! `git log` metadata records (`META_FORMAT`), trailer blocks and cherry-pick references.

use regex::Regex;
use wg_types::{Timestamp, WgError};

/// The `--format` string whose records [`parse_meta`] reads.
pub const META_FORMAT: &str = "%x1e%H%x1f%T%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%(trailers:unfold,separator=%x1d)%x1f%B";

/// One `Key: value` trailer of a commit message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    pub key: String,
    pub value: String,
}

/// One commit as `git log` with [`META_FORMAT`] prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitMeta {
    pub sha: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: Option<Timestamp>,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_at: Option<Timestamp>,
    pub trailers: Vec<Trailer>,
    pub message: String,
}

impl CommitMeta {
    /// The first line of the message, trimmed.
    pub fn subject(&self) -> &str {
        self.message.split('\n').next().unwrap_or_default().trim()
    }

    /// The commit this one was cherry-picked from, when its message says so.
    pub fn cherry_picked_from(&self) -> Option<String> {
        cherry_picked_from(&self.message)
    }
}

/// The sha named by a `(cherry picked from commit <sha>)` line: the first such reference, with
/// 7 to 64 lower-case hex digits.
pub fn cherry_picked_from(message: &str) -> Option<String> {
    // Compiled where it is used: a `static` lazy regex would be interior mutability in a Core crate.
    let Ok(pattern) = Regex::new(r"\(cherry picked from commit ([0-9a-f]{7,64})\)") else {
        return None;
    };
    pattern
        .captures(message)
        .and_then(|caps| caps.get(1))
        .map(|sha| sha.as_str().to_owned())
}

/// Trailers from the `%(trailers:unfold,separator=%x1d)` field. Items that do not look like
/// `Key: value` are skipped.
pub fn parse_trailers(raw: &str) -> Vec<Trailer> {
    let Ok(line) = Regex::new(r"^([A-Za-z0-9][A-Za-z0-9-]*):\s*(.*)$") else {
        return Vec::new();
    };
    raw.split('\x1d')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let caps = line.captures(item)?;
            Some(Trailer {
                key: caps.get(1)?.as_str().to_owned(),
                value: caps.get(2)?.as_str().trim().to_owned(),
            })
        })
        .collect()
}

/// Parses the records of `git log --format=META_FORMAT`. An empty date field is `None`; a date
/// that is not an ISO 8601 timestamp with an offset is an error.
pub fn parse_meta(output: &str) -> Result<Vec<CommitMeta>, WgError> {
    output
        .split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(11, '\x1f').collect();
            let [sha, tree, parents, an, ae, ad, cn, ce, cd, trailers, body] = fields.as_slice()
            else {
                return None;
            };
            Some(parse_date(ad).and_then(|authored_at| {
                parse_date(cd).map(|committed_at| CommitMeta {
                    sha: sha.trim().to_owned(),
                    tree: tree.trim().to_owned(),
                    parents: parents.split_whitespace().map(str::to_owned).collect(),
                    author_name: (*an).to_owned(),
                    author_email: (*ae).to_owned(),
                    authored_at,
                    committer_name: (*cn).to_owned(),
                    committer_email: (*ce).to_owned(),
                    committed_at,
                    trailers: parse_trailers(trailers),
                    message: body.trim_end_matches('\n').to_owned(),
                })
            }))
        })
        .collect()
}

fn parse_date(value: &str) -> Result<Option<Timestamp>, WgError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    Timestamp::parse_iso(value).map(Some)
}
