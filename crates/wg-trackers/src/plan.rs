//! The pure plans of a tracker sync (`TrackerSyncService` in `integrations/trackers/sync.py`):
//! the cursor arithmetic, the references a page still needs, the pull request link changes, the
//! repository list, the reconcile of a full sync and the names a sync writes.
//!
//! The shell reads the rows, asks these functions what to write, and applies the answer in the
//! page's transaction. Nothing here reads or writes a database.

use std::collections::{BTreeSet, HashSet};

use wg_types::Timestamp;

use crate::mapping::TrackerSnapshot;
use crate::pull_request::PullRequestRef;
use crate::sync::project_of;

/// Issues per page: each page is applied in its own transaction (`PAGE_SIZE`).
pub const PAGE_SIZE: usize = 50;

/// The `created_by` of every row a sync writes (`SYNC_ACTOR`).
pub const SYNC_ACTOR: &str = "system:tracker-sync";

/// The `kind` of a pull request task link.
pub const PULL_REQUEST_KIND: &str = "pull_request";

/// The name of a sync cursor row: `tracker:<connection>:<suffix>`.
#[must_use]
pub fn cursor_name(connection: &str, suffix: &str) -> String {
    format!("tracker:{connection}:{suffix}")
}

/// `_ms`: a timestamp in whole epoch milliseconds, as the cursor rows store it.
#[must_use]
pub fn timestamp_ms(at: Timestamp) -> i64 {
    let value = at.to_offset_datetime();
    value.unix_timestamp() * 1000 + i64::from(value.microsecond() / 1000)
}

/// The `since` of an incremental listing, in epoch seconds: the `updated` cursor less the overlap.
/// `None` when there is no cursor yet, so the listing takes every issue.
#[must_use]
pub fn since_seconds(cursor_ms: Option<i64>, overlap_seconds: i64) -> Option<i64> {
    cursor_ms.map(|ms| ms.div_euclid(1000) - overlap_seconds)
}

/// `_write_cursor`: a cursor only moves forward. An unset cursor takes the candidate.
#[must_use]
pub fn advanced_cursor(current: Option<i64>, candidate: i64) -> i64 {
    current.map_or(candidate, |value| value.max(candidate))
}

/// The newest `remote_updated_at` of the snapshots a page searched (not the ones it fetched for
/// references). `None` for an empty page, which writes no cursor.
#[must_use]
pub fn newest_update(searched: &[TrackerSnapshot]) -> Option<Timestamp> {
    searched.iter().map(|snap| snap.remote_updated_at).max()
}

/// The keys a snapshot refers to: its parent, then its blockers.
#[must_use]
pub fn referenced_keys(snap: &TrackerSnapshot) -> Vec<&str> {
    snap.parent_key
        .iter()
        .chain(snap.blocked_by_keys.iter())
        .map(String::as_str)
        .collect()
}

/// The referenced keys worth fetching: not in `have` (the keys already searched or fetched), in a
/// configured project, each once, in first-seen order. The shell drops the ones it already holds.
#[must_use]
pub fn fetch_candidates(
    referenced: &[&str],
    have: &HashSet<String>,
    projects: &[String],
) -> Vec<String> {
    referenced
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, key)| !referenced[..*index].contains(key))
        .filter(|(_, key)| !have.contains(*key))
        .filter(|(_, key)| projects.iter().any(|project| project == project_of(key)))
        .map(|(_, key)| key.to_owned())
        .collect()
}

/// The `(external_id, key)` pairs a full listing returns, reduced to the ids it names.
#[must_use]
pub fn listed_ids(keys: &[(String, String)]) -> HashSet<String> {
    keys.iter()
        .map(|(external_id, _)| external_id.clone())
        .collect()
}

/// The `task.synced` changed fields: sorted and without repeats.
#[must_use]
pub fn changed_fields(changed: &[&str]) -> Vec<String> {
    let unique: BTreeSet<String> = changed.iter().map(|field| (*field).to_owned()).collect();
    unique.into_iter().collect()
}

/// `_project_slug`: the workspace project a Jira project key maps to (`project_key.lower()`).
#[must_use]
pub fn project_slug(project_key: &str) -> String {
    project_key.to_lowercase()
}

/// `_repo_list`: the registered repository ids of the resolved pull request repositories, in
/// first-seen order without duplicates. `None` entries (unregistered repositories) are left out.
#[must_use]
pub fn repository_list(resolved: &[Option<String>]) -> Vec<String> {
    resolved
        .iter()
        .enumerate()
        .filter_map(|(index, repo)| repo.as_deref().map(|id| (index, id)))
        .filter(|(index, id)| {
            !resolved[..*index]
                .iter()
                .any(|earlier| earlier.as_deref() == Some(*id))
        })
        .map(|(_, id)| id.to_owned())
        .collect()
}

/// The reconcile input of one tracked item: whether its task is archived (`None` when the task
/// row is gone) and the mark a previous full sync set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileItem<'a> {
    pub external_id: &'a str,
    pub task_archived: Option<bool>,
    pub missing_since: Option<Timestamp>,
}

/// What a full sync does with one tracked item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    /// Listed, but its task row is gone: nothing changes.
    Untouched,
    /// Listed again. `clear_missing` drops the mark; `unarchive` reopens a task that only the
    /// grace period archived (it had a mark).
    Restored {
        clear_missing: bool,
        unarchive: bool,
    },
    /// Not listed. The mark is set to `since` (the earlier mark, or `now`); `archive` says the
    /// task is live and the grace period has passed.
    Missing { since: Timestamp, archive: bool },
}

/// `_reconcile`: one action per item, in input order. An item the full listing returned is
/// restored; one it did not return is marked, and archived once `now` is more than
/// `grace_seconds` past its mark.
#[must_use]
pub fn reconcile(
    items: &[ReconcileItem],
    listed: &HashSet<String>,
    now: Timestamp,
    grace_seconds: i64,
) -> Vec<ReconcileAction> {
    items
        .iter()
        .map(|item| {
            if listed.contains(item.external_id) {
                match item.task_archived {
                    None => ReconcileAction::Untouched,
                    Some(archived) => ReconcileAction::Restored {
                        clear_missing: item.missing_since.is_some(),
                        unarchive: item.missing_since.is_some() && archived,
                    },
                }
            } else {
                let since = item.missing_since.unwrap_or(now);
                let past_grace = (now.to_offset_datetime() - since.to_offset_datetime())
                    .whole_microseconds()
                    > i128::from(grace_seconds) * 1_000_000;
                ReconcileAction::Missing {
                    since,
                    archive: item.task_archived == Some(false) && past_grace,
                }
            }
        })
        .collect()
}

/// The stored values of a pull request task link, as the sync compares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkValues {
    pub url: String,
    pub provider: String,
    pub repo_full_name: String,
    pub number: Option<i64>,
    pub repository_id: Option<String>,
    pub sources: Vec<String>,
    pub title: String,
}

/// A pull request link row of a task: its normalized URL, who created it and its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLink {
    pub normalized_url: String,
    pub created_by: String,
    pub values: LinkValues,
}

/// One change to the pull request links of a task. `index` names the stored link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkChange {
    Insert {
        normalized_url: String,
        values: LinkValues,
    },
    Update {
        index: usize,
        values: LinkValues,
    },
    Delete {
        index: usize,
    },
}

/// The values a pull request link takes, given the repository it resolved to.
#[must_use]
pub fn link_values(pr: &PullRequestRef, repository_id: Option<&str>) -> LinkValues {
    LinkValues {
        url: pr.url.clone(),
        provider: pr.provider.to_owned(),
        repo_full_name: pr.repo_full_name.clone(),
        number: i64::try_from(pr.number).ok(),
        repository_id: repository_id.map(str::to_owned),
        sources: pr
            .sources
            .iter()
            .map(|source| (*source).to_owned())
            .collect(),
        title: pr.title.clone(),
    }
}

/// `_sync_pull_requests`: the link changes that make a task's pull request links match the
/// snapshot. `wanted` pairs each pull request with the repository id it resolved to. The first
/// reference to a URL wins. Links are matched by normalized URL (the last stored one, as a
/// dictionary keeps). A link the sync created that is no longer named is deleted; links a person
/// created are kept.
#[must_use]
pub fn pull_request_changes(
    stored: &[StoredLink],
    wanted: &[(PullRequestRef, Option<String>)],
) -> Vec<LinkChange> {
    let first_wanted: Vec<(&PullRequestRef, Option<&str>)> = wanted
        .iter()
        .enumerate()
        .filter(|(index, (pr, _))| {
            !wanted[..*index]
                .iter()
                .any(|(earlier, _)| earlier.normalized_url == pr.normalized_url)
        })
        .map(|(_, (pr, repo))| (pr, repo.as_deref()))
        .collect();
    let stored_index = |url: &str| stored.iter().rposition(|link| link.normalized_url == url);
    let upserts = first_wanted.iter().filter_map(|(pr, repo)| {
        let values = link_values(pr, *repo);
        match stored_index(pr.normalized_url.as_str()) {
            None => Some(LinkChange::Insert {
                normalized_url: pr.normalized_url.clone(),
                values,
            }),
            Some(index) if stored[index].values != values => {
                Some(LinkChange::Update { index, values })
            }
            Some(_) => None,
        }
    });
    let wanted_urls: HashSet<&str> = first_wanted
        .iter()
        .map(|(pr, _)| pr.normalized_url.as_str())
        .collect();
    let deletes = stored.iter().enumerate().filter_map(|(index, link)| {
        let kept_by_name = wanted_urls.contains(link.normalized_url.as_str());
        let last_of_url = stored_index(link.normalized_url.as_str()) == Some(index);
        (!kept_by_name && last_of_url && link.created_by == SYNC_ACTOR)
            .then_some(LinkChange::Delete { index })
    });
    upserts.chain(deletes).collect()
}
